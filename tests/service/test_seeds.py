"""Seeds, end to end: an analysis object in a project fills part of a
test's setup (`hertzforge.io.seeds`), the seeded setup is the one the
control service accepted and ran on the simulator, and the run's
converted project holds a specification equal to the source object
within the stated tolerance.

For each seeded module:

1. A project object seeds the module's setup (`seeds.seed`), its
   protective numbers apart, which the operator types (here, applied as
   typed); the setup is a short one the control service runs in seconds.
2. `schema/golden/seeds/<module>.json` holds the seeded configuration,
   its channel table and its imports, the setup before the seed and the
   seed as `project.seed` answers it (which the shell's own test takes), and is exactly what seeding gives
   today (`HERTZFORGE_WRITE_SEEDS=1` rewrites it). The control service's
   own test, `crates/sds-control/tests/seeded_runs.rs`, configures and
   runs each on the simulator and checks the run completes;
   `HERTZFORGE_WRITE_SEEDED_RUNS=1` there writes the run folder to
   `schema/golden/seeded_runs/<module>/`.
3. That run folder was accepted with the seeded configuration, and its
   converted project (`run_convert.convert`) holds the source object's
   requirement within the tolerance `io.setup_convert` states.

A dropped unit conversion fails: with the factor taken out, seeding no
longer gives the configuration the run was accepted with.

All runs are SIMULATED; nothing here is evidence about hardware."""

from __future__ import annotations

import copy
import json
import os
import shutil
from pathlib import Path

import numpy as np
import pytest

from hertzforge import Project
from hertzforge.core.classical_pulse import ClassicalPulse
from hertzforge.core.data import (
    ShockSpecification,
    Specification,
    TimeHistory,
    TransientSpecification,
)
from hertzforge.core.run_record import RunRecord
from hertzforge.core.sine import LOG, SineSweepSpecification, SineTone
from hertzforge.io import run_convert, seeds
from hertzforge.io import setup_convert as sc

ROOT = Path(__file__).resolve().parents[2]
SEEDS = ROOT / 'schema' / 'golden' / 'seeds'
RUNS = ROOT / 'schema' / 'golden' / 'seeded_runs'
G = 9.80665

#: the modules seeded end to end, with the configure request each takes
MODULES = {'random': 'randomConfigureParams', 'sine': 'sineConfigureParams',
           'srs': 'srsConfigureParams', 'classical_shock': 'shockConfigureParams',
           'twr': 'twrConfigureParams', 'mixed': 'mixedConfigureParams',
           'mimo_random': 'mimoSetupParams'}

#: the golden MIMO Random run: its setup is the short one the 4 by 4 plant reaches
MIMO_GOLDEN = ROOT / 'schema' / 'golden' / 'runs' / 'mimo_random'


def _row(index, role, description, sensitivity=100, top=100):
    drive = role == 'drive'
    return {'index': index, 'enabled': True, 'description': description, 'role': role,
            'physical': None, 'interface': 'single_ended', 'iepe_bias_v': None,
            'units': 'V' if drive else 'g', 'sensitivity_mv_per_eu': 1000 if drive else sensitivity,
            'range_max': 10 if drive else top, 'polarity': 'positive', 'control_group': None,
            'abort': {'accel_g': None, 'velocity_ips': None, 'displacement_mil': None}}


def _rows(sensitivity=100, top=100):
    return [_row(1, 'control', 'Control', sensitivity, top),
            _row(2, 'monitor', 'Monitor', sensitivity, top), _row(3, 'drive', 'Drive')]


RECORD = {'operator': 'Operator', 'article': 'Bracket', 'notes': 'a seeded setup',
          'specification': {'document': 'Example procedure', 'clause': '3.1'},
          'record': {'streams': False}}

#: each module's short setup before the seed fills it: the golden runs'
BASES = {
    'random': {
        'title': 'Seeded Random', **RECORD,
        'profile': {'breakpoints': [{'freq_hz': 40, 'psd_g2_hz': 0.01},
                                    {'freq_hz': 200, 'psd_g2_hz': 0.01}],
                    'alarm_db': [3, 3], 'abort_db': [6, 6]},
        'acquisition': {'fs_hz': 1280, 'n_pts': 64, 'n_avg': 2},
        'schedule': {'mode': 'auto', 'levels': [{'db': -3, 'dwell_s': 0.25}, {'db': 0}],
                     'full_level_s': 0.5},
        'control': {'review_after_sysid': False, 'drive_ceiling_vrms': 20,
                    'level_ramp_s': 0.1, 'shutdown_ramp_s': 0.1, 'soft_start_db': 0,
                    'pretest': {'averages': 1}},
        'limits': [], 'feasibility': None},
    'sine': {
        'title': 'Seeded Sine', **RECORD,
        'profile': {'rows': [{'freq_hz': 250, 'quantity': 'accel_g', 'value': 0.2},
                             {'freq_hz': 350, 'quantity': 'accel_g', 'value': 0.2}],
                    'alarm_db': [1.5, 1.5], 'abort_db': [3, 3]},
        'test': {'kind': 'sweep', 'band_hz': [250, 350],
                 'rate': {'law': 'log', 'oct_per_min': 1}, 'start_hz': 250,
                 'direction': 'up', 'end': {'sweeps': 2}},
        'acquisition': {'fs_hz': 4096, 'tick_ms': 100},
        'schedule': {'mode': 'auto', 'levels_db': [-6, 0], 'per_level': {'sweeps': 1},
                     'full_level': {'sweeps': 1}},
        'identification': {'review': False},
        'limits': {'drive_vpk_ceiling': 5, 'drive_vpk_abort': 8}},
    'srs': {
        'title': 'Seeded SRS', **RECORD,
        'target': {'kind': 'flat', 'accel_g': 2},
        'bands': {'f_lo_hz': 200, 'f_hi_hz': 800, 'bands_per_octave': 3, 'q': 10},
        'acquisition': {'fs_hz': 2048, 'ring_out_s': 0.02},
        'schedule': {'mode': 'auto', 'levels_db': [-6, 0], 'pulses': [1, 1],
                     'advance': 'count'},
        'spacing': {'cooling_s': 0.05, 'settling_s': 0, 'compute_s': 0},
        'identification': {'nfft': 256, 'review': False},
        'limits': {'drive_vpk_ceiling': 5, 'drive_vpk_abort': 8}},
    'classical_shock': {
        'title': 'Seeded Classical Shock', **RECORD,
        'pulse': {'shape': 'trapezoid', 'amplitude_g': 3, 'duration_ms': 20,
                  'rise_frac': 0.2, 'fall_frac': 0.2},
        'acquisition': {'fs_hz': 2048, 'srs': {'f_lo_hz': 20, 'f_hi_hz': 800,
                                               'bands_per_octave': 3, 'band_tol_db': 3}},
        'schedule': {'mode': 'auto', 'levels_db': [-6, 0], 'pulses': [1, 1],
                     'advance': 'count'},
        'spacing': {'cooling_s': 0.05, 'settling_s': 0, 'compute_s': 0},
        'identification': {'duration_s': 0.5, 'nfft': 512, 'band_hz': [4, 800],
                           'review': False},
        'control': {'band_hz': [4, 800]},
        'limits': {'drive_vpk_ceiling': 2, 'drive_vpk_abort': 3}},
    'twr': {
        'title': 'Seeded TWR', **RECORD,
        'reference': {'sha256': '0' * 64},
        'prep': {'detrend': 'none', 'highpass_hz': None, 'taper_frac': None},
        'pads': {'pre': 32, 'post': 64},
        'identification': {'drive_vrms': 0.1, 'duration_s': 0.5, 'nfft': 256,
                           'band_hz': [20, 800], 'review': False},
        'schedule': {'mode': 'auto', 'levels_db': [-6, 0], 'gate_pct': 5.5},
        'run': {'schedule': 'once'},
        'limits': {'drive_vpk_ceiling': 5}},
    'mixed': {
        'title': 'Seeded Mixed Mode', **RECORD,
        'parts': [{'id': 'floor', 'kind': 'floor',
                   'breakpoints': [{'freq_hz': 100, 'psd_g2_hz': 0.0005},
                                   {'freq_hz': 500, 'psd_g2_hz': 0.0005}],
                   'alarm_db': [3, 3], 'abort_db': [6, 6]}],
        'exclusion_hz': 32,
        'acquisition': {'fs_hz': 4096, 'n_pts': 256, 'n_avg': 8, 'tick_ms': 100},
        'schedule': {'mode': 'auto', 'levels': [{'db': -6, 'dwell_s': 1}, {'db': 0}],
                     'full_level_s': 1},
        'control': {'pretest': {'averages': 2}, 'floor_settle_s': 1, 'part_ramp_s': 0.5,
                    'level_ramp_s': 0.5, 'shutdown_ramp_s': 0.5, 'review_after_sysid': False},
        'limits': {'drive_vpk_ceiling': 5, 'drive_vpk_abort': 8}},
}

def _mimo_base():
    config = json.loads((MIMO_GOLDEN / 'setup.json').read_text(encoding='utf-8'))['config']
    config = {**config, 'title': 'Seeded MIMO Random', 'record': {'streams': False}}
    # the reference the seed replaces: none of the golden one is kept
    config['reference'] = {'kind': 'inline', 'freqs_hz': [160.0, 166.0], 'n': 4,
                           're': [1e-6 if r == c else 0.0 for _f in range(2)
                                  for r in range(4) for c in range(r, 4)],
                           'im': [0.0] * 20}
    return config


BASES['mimo_random'] = _mimo_base()

TABLES = {'random': _rows(), 'sine': _rows(), 'srs': _rows(1000, 10),
          'classical_shock': _rows(1000, 10), 'twr': _rows(), 'mixed': _rows(),
          'mimo_random': json.loads((MIMO_GOLDEN / 'table.json').read_text(
              encoding='utf-8'))['rows']}


# ---- the source objects, written as the analysis side writes them ----------------------------

#: the significant digits a source's stored numbers are settled to
SETTLED_DIGITS = 7


def _settle(values):
    """`values` with every number written to `SETTLED_DIGITS` significant
    digits and read back. A source's samples come from `np.sin`, `np.exp`
    and powers of ten, whose last bit differs between the platforms' math
    libraries; the object's SHA-256 (the seed's origin) is of its bytes, so
    the stored numbers must be the same bytes everywhere. Decimal formatting
    is correctly rounded on every platform, and a last-bit difference does
    not survive seven digits."""
    a = np.asarray(values)

    def fixed(x):
        return np.array([float(f'{v:.{SETTLED_DIGITS - 1}e}') for v in np.ravel(x)]).reshape(
            np.shape(x))

    if np.iscomplexobj(a):
        return fixed(a.real) + 1j * fixed(a.imag)
    return fixed(a)


#: the arrays settled, by whether the object is a time history
_SETTLED_ARRAYS = {False: ('abscissa', 'ordinate', 'frequency', 'amplitude', 'phase',
                           'segment_rate'),
                   True: ('ordinate',)}


def _settled(make):
    """A source builder whose object stores only settled numbers (`_settle`)."""
    import functools

    @functools.wraps(make)
    def build():
        obj = make()
        for o in [obj, *getattr(obj, 'tones', [])]:
            # a time history's clock is k / rate, exact arithmetic that is the same
            # everywhere, and rounding it would make its steps uneven
            for name in _SETTLED_ARRAYS[isinstance(o, TimeHistory)]:
                if isinstance(getattr(o, name, None), np.ndarray):
                    setattr(o, name, _settle(getattr(o, name)))
            for key, value in list(getattr(o, 'limits', {}).items()):
                o.limits[key] = _settle(value)
        return obj

    return build

@_settled
def _random_spec():
    f = np.array([40.0, 80.0, 160.0, 200.0])
    level = np.array([0.01, 0.02, 0.02, 0.016]) * G ** 2
    return Specification(
        abscissa=f, ordinate=level[np.newaxis], response_dof=['1X+'],
        ordinate_dim=['acceleration**2/frequency'], ordinate_unit=['g**2/Hz'],
        warning_lower=level[np.newaxis] * 10 ** -0.2, warning_upper=level[np.newaxis] * 10 ** 0.2,
        abort_lower=level[np.newaxis] * 10 ** -0.4, abort_upper=level[np.newaxis] * 10 ** 0.5)


@_settled
def _sine_spec():
    amplitude = np.full((2, 1), 0.3 * G)
    tone = SineTone('Sweep', 0.0, [300.0, 320.0], amplitude, [LOG], [3.0],
                    warning_lower=amplitude * 10 ** (-2 / 20), warning_upper=amplitude * 10 ** (2 / 20),
                    abort_lower=amplitude * 10 ** (-4 / 20), abort_upper=amplitude * 10 ** (4 / 20))
    return SineSweepSpecification([tone], ['1Z+'], ordinate_dim='acceleration',
                                  ordinate_unit='m/s**2')


@_settled
def _srs_spec():
    return ShockSpecification.from_breakpoints(
        [200.0, 800.0], [1.5, 3.0], '1Z+', per_octave=3, q=10.0, ordinate_unit='g',
        warning=(-3.0, 3.0), abort=(-9.0, 6.0))


PULSE = ClassicalPulse('half-sine', 2.0 * G, 0.011)


@_settled
def _pulse_spec():
    time, values = sc.pulse_waveform(PULSE, 51200.0)
    return TransientSpecification(time, values[np.newaxis], response_dof=['1Z+'],
                                  ordinate_dim=['acceleration'], ordinate_unit=['m/s**2'])


TWR_FS = 2048.0


@_settled
def _twr_history():
    t = np.arange(160) / TWR_FS

    def tone(f, tau, a):
        return a * np.exp(-t / tau) * np.sin(2 * np.pi * f * t)

    values = (tone(150.0, 0.03, 0.4) + tone(330.0, 0.02, 0.3)) * G
    return TimeHistory(t, values[np.newaxis], response_dof=['1Z+'],
                       ordinate_dim=['acceleration'], ordinate_unit=['m/s**2'])


@_settled
def _mixed_floor():
    from hertzforge.core.mixed_mode import Narrowband

    f = np.array([100.0, 500.0])
    level = np.full(2, 0.0005 * G ** 2)
    floor = Specification(
        abscissa=f, ordinate=level[np.newaxis], response_dof=['1Z+'],
        ordinate_dim=['acceleration**2/frequency'], ordinate_unit=['g**2/Hz'],
        warning_lower=level[np.newaxis] * 10 ** -0.3, warning_upper=level[np.newaxis] * 10 ** 0.3,
        abort_lower=level[np.newaxis] * 10 ** -0.6, abort_upper=level[np.newaxis] * 10 ** 0.6)
    floor.narrowbands = (Narrowband(350.0, 0.002 * G ** 2, width=32.0, sweep_to=420.0,
                                    rate=20.0, name='Mesh'),)
    floor.tone_exclusion = 32.0
    return floor


@_settled
def _mixed_tones():
    from hertzforge.core.mixed_mode import Tone

    return SineSweepSpecification([Tone(200.0, 0.5 * G, name='Blade').sine_tone(4.0, 1)],
                                  ['1Z+'], ordinate_dim='acceleration', ordinate_unit='m/s**2')


def _mimo_spec():
    """A specification with cross terms the 4 by 4 plant reaches: the reference
    of the golden MIMO Random run, as its converted project holds it (in SI, in
    the file's convention)."""
    import tempfile

    with tempfile.TemporaryDirectory() as scratch:
        folder = os.path.join(scratch, 'mimo_random')
        shutil.copytree(MIMO_GOLDEN, folder)
        project = Project.open(run_convert.convert(folder).path)
        found = [obj for _name, obj in project.items() if type(obj) is Specification]
    assert len(found) == 1
    run = found[0]
    # on half-hertz lines across the run's band, each a mix of its two neighbors
    # (still a spectral matrix): enough points to be given inline
    f = np.arange(run.abscissa[0], run.abscissa[-1] + 0.25, 0.5)

    def across(values):
        values = np.asarray(values)
        return np.array([np.interp(f, run.abscissa, np.real(v)) + 1j * np.interp(
            f, run.abscissa, np.imag(v)) for v in values])

    limits = {k: np.real(across(v)) for k, v in run.limits.items()}
    return Specification(abscissa=f, ordinate=across(run.ordinate),
                         response_dof=list(run.response_dof),
                         reference_dof=list(run.reference_dof),
                         ordinate_dim=list(run.ordinate_dim),
                         ordinate_unit=list(run.ordinate_unit),
                         reference_unit=list(run.reference_unit), **limits)


SOURCES = {'random': ('Floor Spec', _random_spec), 'sine': ('Sweep Spec', _sine_spec),
           'srs': ('Shock Spec', _srs_spec), 'classical_shock': ('Pulse', _pulse_spec),
           'twr': ('Field Record', _twr_history), 'mixed': ('Floor', _mixed_floor),
           'mimo_random': ('Field Reference', _mimo_spec)}


def _project(module):
    name, make = SOURCES[module]
    project = Project('seeds')
    project.add(name, make())
    if module == 'mixed':
        project.add('Tones', _mixed_tones())
        project.link(name, 'Tones')
    return project, name


def seeded(module):
    """The module's seeded setup: its configuration with the seed's sets
    and its protective numbers as the operator typed them, its imports,
    and the seed."""
    project, name = _project(module)
    found = seeds.seed(project, name, module,
                       tones=seeds.offers(project, module)[0].get('tones'))
    typed = [{'path': p['path'], 'value': p['value']} for p in found['protective']]
    config = seeds.apply(BASES[module], found['sets'] + typed)
    return {'config': config, 'channels': TABLES[module], 'imports': found['imports'],
            'base': BASES[module], 'answer': found}, found


def _on_file(module):
    return json.loads((SEEDS / f'{module}.json').read_text(encoding='utf-8'))


@pytest.mark.parametrize('module', MODULES)
def test_the_seeded_setup_on_file_is_what_seeding_gives(module):
    made, found = seeded(module)
    if os.environ.get('HERTZFORGE_WRITE_SEEDS') == '1':
        SEEDS.mkdir(parents=True, exist_ok=True)
        (SEEDS / f'{module}.json').write_text(json.dumps(made, indent=1) + '\n',
                                              encoding='utf-8')
    assert _on_file(module) == json.loads(json.dumps(made))
    assert found['origin']['kind'] == 'seed' and found['origin']['object'] == SOURCES[module][0]
    assert len(found['origin']['checksum']) == 64


@pytest.mark.parametrize('module', MODULES)
def test_the_seeded_setup_is_a_configure_request(module, validate):
    member = 'setup' if module == 'mimo_random' else 'config'
    validate({member: seeded(module)[0]['config']}, MODULES[module])


@pytest.mark.parametrize('module', ['random', 'sine', 'srs'])
def test_a_protective_number_is_never_among_the_sets(module):
    _made, found = seeded(module)
    paths = [s['path'] for s in found['sets']]
    assert found['protective'], 'the source gives an abort band'
    for guard in found['protective']:
        assert 'abort' in guard['path'] and guard['path'] not in paths
        assert guard['words']
    assert not any('abort' in p for p in paths)


def test_the_checksum_names_the_version_of_the_object():
    a, b = _random_spec(), _random_spec()
    assert seeds.checksum(a) == seeds.checksum(b)
    b.ordinate[0, 0] *= 1.0001
    assert seeds.checksum(a) != seeds.checksum(b)


def test_what_cannot_seed_is_refused_with_the_reason():
    project, name = _project('sine')
    with pytest.raises(seeds.SeedRefused, match='does not seed a random setup'):
        seeds.seed(project, name, 'random')
    with pytest.raises(seeds.SeedRefused, match='nothing seeds'):
        seeds.seed(project, name, 'explorer')
    project.add('Ringing', TransientSpecification(
        np.arange(400) / 20000.0, np.sin(np.arange(400) / 5.0)[np.newaxis] * np.exp(
            -np.arange(400) / 100.0), response_dof=['1Z+'], ordinate_dim=['acceleration']))
    with pytest.raises(seeds.SeedRefused, match='no classical pulse'):
        seeds.seed(project, 'Ringing', 'classical_shock')


def test_the_offers_list_what_a_module_takes():
    project, _name = _project('random')
    project.add('Shock Spec', _srs_spec())
    assert [o['name'] for o in seeds.offers(project, 'random')] == ['Floor Spec']
    assert [o['name'] for o in seeds.offers(project, 'srs')] == ['Shock Spec']
    assert seeds.offers(project, 'explorer') == []


def test_apply_puts_each_value_at_its_path_whole():
    out = seeds.apply({'a': {'b': [1, {'c': 2}]}, 'k': 1},
                      [{'path': 'a.b.1.c', 'value': 3}, {'path': 'x.y', 'value': [1]},
                       {'path': 'k', 'value': {'z': 1}}])
    assert out == {'a': {'b': [1, {'c': 3}]}, 'k': {'z': 1}, 'x': {'y': [1]}}


# ---- the proof: the run on the simulator holds the source ------------------------------------

def _accepted(folder):
    setup = json.loads((folder / 'setup.json').read_text(encoding='utf-8'))
    return setup.get('config') or setup


@pytest.fixture(scope='module')
def converted(tmp_path_factory):
    out = {}
    for module in MODULES:
        folder = tmp_path_factory.mktemp('runs') / module
        shutil.copytree(RUNS / module, folder)
        out[module] = (folder, Project.open(run_convert.convert(str(folder)).path))
    return out


def _without_reference(config):
    config = copy.deepcopy(config)
    config.pop('reference', None)
    return config


@pytest.mark.parametrize('module', MODULES)
def test_the_run_was_accepted_with_the_seeded_setup(module, converted):
    folder, _project_file = converted[module]
    made = seeded(module)[0]['config']
    accepted = _accepted(folder)
    for key, value in made.items():
        if key == 'reference' and module == 'twr':
            # TWR's reference is the control service's store, by the SHA-256 it gave
            continue
        assert _same(accepted.get(key), json.loads(json.dumps(value))), key


def _same(a, b):
    """Equal, numbers to 1e-12: the control service reads a JSON number to
    within its last bit, and writes back what it read."""
    if isinstance(a, dict) and isinstance(b, dict):
        return a.keys() == b.keys() and all(_same(a[k], b[k]) for k in a)
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(_same(x, y) for x, y in zip(a, b, strict=True))
    if isinstance(a, (int, float)) and isinstance(b, (int, float)) and not isinstance(
            a, bool) and not isinstance(b, bool):
        return bool(np.isclose(a, b, rtol=1e-12, atol=1e-300))
    return a == b

def _one(project, kind):
    found = [obj for _name, obj in project.items() if type(obj) is kind]
    assert len(found) == 1, f'{len(found)} {kind.__name__} objects'
    return found[0]


def _loglog(x, xp, fp):
    return np.exp(np.interp(np.log(x), np.log(xp), np.log(fp)))


def test_random_the_run_holds_the_source_specification(converted):
    _folder, project = converted['random']
    got, want = _one(project, Specification), _random_spec()
    assert np.allclose(got.abscissa, want.abscissa, rtol=sc.TOLERANCE, atol=0)
    assert np.allclose(np.real(got.ordinate[0]), np.real(want.ordinate[0]), rtol=sc.TOLERANCE,
                       atol=0)
    for limit in ('warning_lower', 'warning_upper', 'abort_lower', 'abort_upper'):
        assert np.allclose(np.real(got.limits[limit][0]), np.real(want.limits[limit][0]),
                           rtol=1e-4, atol=0), limit


def test_sine_the_run_holds_the_source_tone(converted):
    _folder, project = converted['sine']
    got, want = _one(project, SineSweepSpecification), _sine_spec()
    gt, wt = got.tones[0], want.tones[0]
    assert np.allclose(gt.amplitude, wt.amplitude[0, 0], rtol=sc.TOLERANCE, atol=0)
    assert int(gt.segment_type[0]) == LOG and gt.segment_rate[0] == pytest.approx(3.0)
    assert gt.frequency.min() == pytest.approx(300.0) and gt.frequency.max() == pytest.approx(320.0)
    for limit in ('warning_lower', 'warning_upper', 'abort_lower', 'abort_upper'):
        assert np.allclose(gt.limits[limit], wt.limits[limit][0, 0], rtol=1e-4, atol=0), limit


def test_srs_the_run_holds_the_source_shock_specification(converted):
    _folder, project = converted['srs']
    got, want = _one(project, ShockSpecification), _srs_spec()
    assert got.q == want.q
    expected = _loglog(got.abscissa, want.abscissa, np.real(want.ordinate[0]))
    assert np.allclose(np.real(got.ordinate[0]), expected, rtol=sc.TOLERANCE, atol=0)


def test_classical_shock_the_run_holds_the_source_pulse(converted):
    _folder, project = converted['classical_shock']
    record = _one(project, RunRecord)
    pulse = sc.classical_pulse(record.config['pulse'])
    want = _pulse_spec()
    values = pulse.at(want.abscissa)
    off = np.max(np.abs(values - np.real(want.ordinate[0]))) / abs(PULSE.peak)
    assert off <= sc.PULSE_TOLERANCE
    assert pulse.shape == PULSE.shape


def test_twr_the_run_holds_the_source_record(converted):
    _folder, project = converted['twr']
    got, want = _one(project, TransientSpecification), _twr_history()
    pads = BASES['twr']['pads']
    samples = want.ordinate.shape[1]
    # the prepared section between its pads, which are quiet
    assert got.ordinate.shape[1] == pads['pre'] + samples + pads['post']
    section = np.real(got.ordinate[0, pads['pre']:pads['pre'] + samples])
    peak = np.max(np.abs(want.ordinate[0]))
    assert np.max(np.abs(section - np.real(want.ordinate[0]))) <= sc.TOLERANCE * peak
    assert np.max(np.abs(np.real(got.ordinate[0, :pads['pre']]))) <= sc.TOLERANCE * peak


def test_mixed_the_run_holds_the_source_floor_narrowbands_and_tones(converted):
    from hertzforge.core.mixed_mode import Tone

    _folder, project = converted['mixed']
    floor = _one(project, Specification)
    want = _mixed_floor()
    assert np.allclose(np.real(floor.ordinate[0]), np.real(want.ordinate[0]), rtol=sc.TOLERANCE,
                       atol=0)
    (band,), (source,) = floor.narrowbands, want.narrowbands
    assert band.center == pytest.approx(source.center) and band.sweep_to == source.sweep_to
    assert band.level == pytest.approx(source.level, rel=sc.TOLERANCE)
    assert band.width == source.width and band.rate == source.rate
    assert floor.tone_exclusion == want.tone_exclusion
    (tone,) = _one(project, SineSweepSpecification).tones
    got, expected = Tone.of(tone), Tone.of(_mixed_tones().tones[0])
    assert got.frequency == pytest.approx(expected.frequency)
    assert got.amplitude == pytest.approx(expected.amplitude, rel=sc.TOLERANCE)
    assert got.fixed and expected.fixed


def test_mimo_random_the_run_holds_the_source_reference_with_its_cross_terms(converted):
    _folder, project = converted['mimo_random']
    got, source = _one(project, Specification), _mimo_spec()
    assert got.num_records == source.num_records == 10
    # the run's lines are the source's at every other point
    lines = [int(np.argmin(np.abs(source.abscissa - f))) for f in got.abscissa]
    assert np.allclose(source.abscissa[lines], got.abscissa, rtol=sc.TOLERANCE, atol=0)
    want = source
    want_ordinate = source.ordinate[:, lines]
    assert list(got.response_dof) == list(want.response_dof)
    assert list(got.reference_dof) == list(want.reference_dof)
    peak = np.max(np.abs(want.ordinate))
    # every term, cross terms with their phase, in the file's convention
    assert np.max(np.abs(got.ordinate - want_ordinate)) <= sc.TOLERANCE * peak
    cross = [i for i in range(want.num_records) if want.response_dof[i] != want.reference_dof[i]]
    assert cross and np.max(np.abs(np.imag(want.ordinate[cross]))) > 1e-3 * peak


# ---- falsification: a dropped unit conversion fails ------------------------------------------

@pytest.mark.parametrize('module', MODULES)
def test_a_dropped_unit_conversion_no_longer_gives_the_run_setup(module, monkeypatch,
                                                                converted):
    folder, _project_file = converted[module]
    if module == 'twr':
        monkeypatch.setattr(sc, 'si_transform', lambda unit, dimension=None: (1.0, 0.0))
        made = seeded(module)[0]
        assert made['imports'][0]['params']['text'] != _on_file(module)['imports'][0]['params'][
            'text']
        return
    if module == 'mimo_random':
        monkeypatch.setattr(sc, 'si_transform', lambda unit, dimension=None: (1.0, 0.0))
    monkeypatch.setattr(sc, 'G', 1.0)
    monkeypatch.setitem(sc.SINE_QUANTITIES, 'acceleration', ('accel_g', 1.0))
    made = seeded(module)[0]['config']
    accepted = _accepted(folder)
    kept = made if module == 'mimo_random' else _without_reference(made)
    assert any(accepted.get(k) != json.loads(json.dumps(v)) for k, v in kept.items())


# ---- the verbs -------------------------------------------------------------------------------

def test_the_verbs_answer_as_the_schema_says(validate):
    from hertzforge.service import projects
    from hertzforge.service.protocol import ProtocolError

    project, name = _project('random')
    state = projects.OpenProject(project)
    validate({'module': 'random'}, 'projectSeedsParams')
    listed = projects.seeds(state, {'module': 'random'})
    validate(listed, 'projectSeedsResult')
    assert [s['name'] for s in listed['seeds']] == [name]
    assert projects.seeds(None, {'module': 'random'}) == {'seeds': []}
    params = {'name': name, 'module': 'random'}
    validate(params, 'projectSeedParams')
    answer = projects.seed(state, params)
    validate(answer, 'projectSeedResult')
    assert not state.modified, 'a seed changes nothing in the project'
    with pytest.raises(ProtocolError) as refused:
        projects.seed(state, {'name': name, 'module': 'sine'})
    assert refused.value.code == 'refused' and 'does not seed a sine setup' in str(
        refused.value)
    with pytest.raises(ProtocolError) as missing:
        projects.seed(state, {'name': 'Nothing', 'module': 'random'})
    assert missing.value.code == 'not_found'
    with pytest.raises(ProtocolError):
        projects.seed(state, {'name': name, 'module': 'random', 'record': 'one'})
    with pytest.raises(ProtocolError):
        projects.seed(None, params)


@pytest.mark.parametrize('module', [m for m in MODULES if m != 'mimo_random'])
def test_a_source_stores_only_settled_numbers(module):
    """Its SHA-256 is then the same on every platform: no last bit of a
    platform's sin, exp or pow survives in it. (MIMO Random's source is read
    from the golden run's files and mixed linearly, with no such function.)"""
    obj = SOURCES[module][1]()
    for o in [obj, *getattr(obj, 'tones', [])]:
        arrays = [getattr(o, n) for n in _SETTLED_ARRAYS[isinstance(o, TimeHistory)]
                  if isinstance(getattr(o, n, None), np.ndarray)]
        for values in arrays + list(getattr(o, 'limits', {}).values()):
            assert np.array_equal(_settle(values), values)
