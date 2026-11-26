"""Checks of the MIMO Sine benchmark runner (`msnb_run`) on real Open Benchmark Set structures.

    MSNB_RUN=<msnb_run build> OBS_PLAY=<obs_play build> MSNB_PROBLEMS=<problem folder> \
        python3 check_msnb_run.py [-v]
    MSNB_PROBLEMS=<problem folder> python3 check_msnb_run.py case <problem id> '<test json>' <amplitude> <out.json> ['<extra json>']

Builds its cases in a temporary folder and runs them through the program, at most four at once
(each run is one process on one core). Every check is also run on a case built to break it and
must fail there: a check that cannot fail checks nothing.

1. Determinism: the same case run twice gives the same bytes (run.json, the law's
   identification, the recording). Broken case: a different identification seed.
2. The run file agrees with the open-loop player replaying the recorded drive: the samples
   exactly (once rounded to the station's 32-bit samples), the tracked controls of an update
   (a demodulation of the replay over the half second the update ends, in the steady dwell), and
   the modes' frequencies (the player's envelopes through `reference_model.level_state`).
   Broken cases: the replay with the triangle hold in place of the structure's step; a run file
   whose controls read 0.086 dB high; the envelopes against doubled reference amplitudes.
3. A linear structure with --ideal-frf completes a slow dwell within the engine's alarm band at
   every judged update, and the ideal response's turn leaves no delay. Broken case: the
   unreachable reference of check 4.
4. A reference the drives cannot reach ends as the engine says it should: the drive held at its
   ceiling, the controls under their abort band, a control abort, the drive ramped out. Broken
   case: check 3's completed run.
5. The runner refuses what it must (unknown keys, a reference that does not cover the test) with
   exit 2 and no folder; the set's largest rig (24x12) is taken by the engine since Hertzforge#465.
6. A down sweep starts at the top of the band and falls.
7. A drive gain lets a reference whose ideal drive is over the converter's 10 V be met: a linear
   dwell whose ideal drive peaks near 30 V, with the gain set to that peak (so 1 V in engine
   volts) and the ceiling 6 dB over it, completes within the alarm band with the drive near 1 V;
   the run file states the gain. Broken case: the same dwell with no gain (the drive held at its
   9 V ceiling, the controls under).
8. The thinning of a stepped sine and a dwell (FORMAT_RUN.md, "Thinning"): the same case run
   thinned (the default) and with --whole-history; the thinned file is the whole one thinned by
   the rule as FORMAT_RUN.md states it, restated here: every judged update a point of its own with
   the whole file's values, the other updates' spans ending where their key changes, the counts,
   and each span's drive and mode extremes (null on a point standing for one update); everything
   outside the history the same. Broken cases: a judged update dropped; each span's largest drive
   1e-3 low; a drive extreme written on a point standing for one update; each span's mode maximum
   1e-6 low.

Checks 1 and 2 read the recorded dwell update by update, so it runs with --whole-history; the
other runs are thinned as the runner writes by default.

The structures are read from MSNB_PROBLEMS and never written. reference_model.py is the one in
the problem's own folder.
"""
import concurrent.futures as cf
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

import numpy as np

RUN = os.environ.get('MSNB_RUN')
PLAY = os.environ.get('OBS_PLAY')
PROBLEMS = os.environ.get('MSNB_PROBLEMS')
FS = 8192.0
REALISTIC = 'frame-6x6-realistic_s00'
LINEAR = 'frame-6x6_s00'
BIG = 'frame-24x12-realistic_s00'


def reference_model(pid):
    spec = importlib.util.spec_from_file_location('reference_model_' + pid.replace('-', '_'),
                                                  os.path.join(PROBLEMS, pid, 'reference_model.py'))
    rm = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(rm)
    return rm, rm.load_structure(os.path.join(PROBLEMS, pid, 'structure.json'))


def reachable_reference(pid, amp, band, per_octave=48):
    """In phase at every control, projected onto what the rig reaches (least-squares drive through
    the small-signal response, its response at the controls)."""
    rm, st = reference_model(pid)
    rows = [i for i, o in enumerate(st.raw['outputs']) if o['role'] == 'control']
    lo, hi = band
    f = np.geomspace(lo, hi, int(np.ceil(np.log2(hi / lo) * per_octave)) + 1)
    H = rm.frf(st, f, rows=rows)
    want = np.full(len(rows), amp, complex)
    got = np.array([H[k] @ np.linalg.lstsq(H[k], want, rcond=None)[0] for k in range(len(f))])
    ph = np.angle(got, deg=True)
    ph[ph <= -180.0] += 360.0
    return {'schema': 'msnb-reference/1', 'controls': [st.raw['outputs'][i]['name'] for i in rows],
            'frequency_hz': f.tolist(), 'amplitude_m_s2_pk': np.abs(got).tolist(), 'phase_deg': ph.tolist()}


def case(pid, test, amp, band=None, **extra):
    band = band or ((test['freq_hz'] / 1.5, test['freq_hz'] * 1.5) if test['type'] == 'dwell' else tuple(test['band_hz']))
    c = {'schema': 'msnb-case/1', 'case_id': f'{pid}-{test["type"]}', 'structure': {'file': f'{pid}/structure.json'},
         'law': {'id': 'matrix'}, 'test': test, 'reference': {'inline': reachable_reference(pid, amp, band)}}
    c.update(extra)
    return c


def run(tmp, name, c, *flags):
    path = os.path.join(tmp, name + '.json')
    with open(path, 'w') as fh:
        json.dump(c, fh)
    out = os.path.join(tmp, name)
    p = subprocess.run([RUN, path, '--root', PROBLEMS, '--out', out, *flags], capture_output=True, text=True)
    return p.returncode, out, p.stderr


def load(out):
    with open(os.path.join(out, 'run.json')) as fh:
        return json.load(fh)


def cplx(h, key, k):
    return np.array(h[key + '_re'][k]) + 1j * np.array(h[key + '_im'][k])


# ---- the checks, each a function that returns a list of failures (empty: it holds) ----------

def same_bytes(a, b, names=('run.json', 'law_identification.f8', 'record/drive.f8', 'record/inputs.f8')):
    bad = []
    for n in names:
        with open(os.path.join(a, n), 'rb') as fa, open(os.path.join(b, n), 'rb') as fb:
            if fa.read() != fb.read():
                bad.append(f'{n} differs')
    return bad


def replay_matches(out, step=None):
    rs = load(out)['record']
    rep = os.path.join(out, 'replay.f8')
    args = [PLAY, os.path.join(out, 'record', 'structure.json'), '--drive', os.path.join(out, 'record', 'drive.f8'),
            '--out', rep, '--fs', str(FS)]
    if step:
        args += ['--step', step]
    subprocess.run(args, check=True, capture_output=True)
    a = np.fromfile(rep).reshape(-1, rs['channels'])
    b = np.fromfile(os.path.join(out, 'record', 'inputs.f8')).reshape(-1, rs['channels'])
    os.remove(rep)
    if a.shape != b.shape:
        return [f'replay {a.shape} against record {b.shape}']
    d = np.abs(a.astype(np.float32).astype(np.float64) - b).max()
    return [] if d == 0.0 else [f'largest difference {d} m/s^2']


def phasors_match(out, freq_hz, scale=1.0, tol_db=0.01, tol_deg=0.01):
    r = load(out)
    h = r['history']
    nc = len(r['structure']['controls'])
    x = np.fromfile(os.path.join(out, 'record', 'inputs.f8')).reshape(-1, nc + len(r['structure']['limits']))[:, :nc]
    n = int(FS * 0.5)                                   # whole periods at 150 Hz
    # Updates where the drive has changed by under 1e-4 of itself (0.001 dB) over the last 0.75 s:
    # the response is steady over the demodulation's half second, whatever the engine's tracker is.
    d = np.array(h['drive_re']) + 1j * np.array(h['drive_im'])
    lag = int(round(0.75 / r['structure']['tick_s']))
    steady = [k for k in range(lag, len(d) - 1)
              if np.abs(d[k - lag:k + 1] - d[k]).max() <= 1e-4 * np.abs(d[k]).max()]
    if not steady:
        return ['no update with a steady drive']
    bad = []
    for k in (steady[0], steady[-1]):
        end = int(round(h['time_s'][k] * FS))
        t = np.arange(end - n, end) / FS
        ph = 2.0 * (x[end - n:end] * np.exp(-2j * np.pi * freq_hz * t)[:, None]).mean(0)
        c = cplx(h, 'control', k) * scale
        db = np.abs(20 * np.log10(np.abs(c) / np.abs(ph))).max()
        deg = np.abs(np.angle((c / c[0]) / (ph / ph[0]), deg=True)).max()
        if db > tol_db or deg > tol_deg:
            bad.append(f'update {h["index"][k]}: {db:.4g} dB, {deg:.4g} degrees')
    return bad


def modes_match(out, pid, amp_scale=1.0, tol=1e-7):
    r = load(out)
    tm = r['true_modes']
    if not tm['tracked']:
        return ['not tracked']
    rm, st = reference_model(pid)
    env_path = os.path.join(out, 'env.f8')
    subprocess.run([PLAY, os.path.join(out, 'record', 'structure.json'), '--drive', os.path.join(out, 'record', 'drive.f8'),
                    '--out', os.path.join(out, 'replay.f8'), '--env', env_path, '--fs', str(FS)], check=True, capture_output=True)
    env = np.fromfile(env_path).reshape(-1, len(st.raw['modes']))
    os.remove(env_path)
    os.remove(os.path.join(out, 'replay.f8'))
    ra = np.array([m['reference_amplitude'] for m in st.raw['modes']]) * amp_scale
    idx = [m['index'] for m in tm['modes']]
    worst = 0.0
    for k, t in enumerate(r['history']['time_s']):
        s = int(round(t * FS)) // tm['resolution_samples'] * tm['resolution_samples']
        if s == 0:
            continue
        f, _ = rm.level_state(st, env[s - 1] / ra)
        worst = max(worst, float(np.max(np.abs(f[idx] - np.array(tm['frequency_hz'][k])) / f[idx])))
    return [] if worst <= tol else [f'largest relative difference {worst:.3g}']


def completes_within_alarm(out):
    r = load(out)
    bad = []
    if r['outcome']['class'] != 'completed':
        bad.append(f'ended {r["outcome"]["class"]}')
    h = r['history']
    alarm = r['settings']['alarm_db']
    worst = max((abs(e) for k, j in enumerate(h['judged']) if j for e in h['error_db'][k]), default=float('inf'))
    if not worst <= alarm:
        bad.append(f'worst judged error {worst:.4g} dB over {alarm} dB')
    if r['summary']['judged_updates'] < 100:
        bad.append(f'{r["summary"]["judged_updates"]} judged updates')
    d = r['identification']['ideal_vs_measured']['fitted_delay_samples']
    if d is None or abs(d) > 0.5:
        bad.append(f'a delay of {d} samples left between the ideal and the measured response')
    return bad


def ideal_peak(pid, test, amp):
    """The least-squares drive's largest size over the reference's lines near the dwell, V."""
    rm, st = reference_model(pid)
    rows = [i for i, o in enumerate(st.raw['outputs']) if o['role'] == 'control']
    ref = reachable_reference(pid, amp, (test['freq_hz'] / 1.5, test['freq_hz'] * 1.5))
    f = np.array(ref['frequency_hz'])
    want = np.array(ref['amplitude_m_s2_pk']) * np.exp(1j * np.radians(np.array(ref['phase_deg'])))
    H = rm.frf(st, f, rows=rows)
    return float(max(np.abs(np.linalg.lstsq(H[k], want[k], rcond=None)[0]).max() for k in range(len(f))))


def gain_met(out, gain, ceiling):
    r = load(out)
    bad = completes_within_alarm(out)
    if r['settings'].get('drive_gain') != gain:
        bad.append(f'the run file states the gain as {r["settings"].get("drive_gain")}')
    if not r['summary']['largest_drive_vpk'] <= ceiling + 1e-9:
        bad.append(f'the drive reached {r["summary"]["largest_drive_vpk"]} V over the {ceiling} V ceiling')
    return bad


def ends_as_the_engine_says(out, ceiling=9.0):
    r = load(out)
    o, s, h = r['outcome'], r['summary'], r['history']
    bad = []
    if o['class'] != 'stopped_by_protective_default' or o['guard'] != 'control_abort':
        bad.append(f'ended {o["class"]} / {o["guard"]}')
    if not s['largest_sample_v'] <= ceiling + 1e-6:
        bad.append(f'a sample at {s["largest_sample_v"]} V over the {ceiling} V ceiling')
    if abs(s['largest_drive_vpk'] - ceiling) > 1e-6:
        bad.append(f'the drive reached {s["largest_drive_vpk"]} V, not its ceiling')
    if not s['drive_is_zero']:
        bad.append('the drive was not ramped out')
    last = h['error_db'][-1]
    if o['channel'] is not None:
        k = r['structure']['controls'].index(o['channel'])
        if not last[k] < -r['settings']['abort_db']:
            bad.append(f'the named control at {last[k]} dB, inside its abort band')
    if not any(e['kind'] == 'over_ceiling' for e in r['events']):
        bad.append('no over_ceiling event')
    return bad


def n9(x):
    return float(f'{x:.8e}')


def thin_spans(h):
    """FORMAT_RUN.md's stepped and dwell thinning, restated from the text: every judged update a point standing for
    itself; of the others, the first and the last update of each run of consecutive updates with the same key (level
    rounded to 1e-6 dB, frequency, settling, miss); a point stands for the updates up to the next point."""
    n = len(h['index'])
    key = [(round(h['level_db'][i] * 1e6), h['frequency_hz'][i], h['settling'][i], h['miss'][i]) for i in range(n)]
    j = h['judged']

    def same(a, b):
        return not j[a] and not j[b] and key[a] == key[b]
    pts = [i for i in range(n) if i == 0 or i == n - 1 or not same(i - 1, i) or not same(i, i + 1)]
    return list(zip(pts, pts[1:] + [n]))


def thinned_as_stated(whole, thin, rel=1e-8):
    """The thinned run file `thin` (a dict) against the whole one: a list of what differs. The drive extremes are
    magnitudes written to nine digits, so they are compared within 1e-8 of themselves."""
    bad = []
    hw, ht = whole['history'], thin['history']
    if hw['decimation'] != 'every update' or ht['decimation'] != 'every judged update':
        return [f'decimation {hw["decimation"]!r} and {ht["decimation"]!r}']
    spans = thin_spans(hw)
    if ht['index'] != [hw['index'][s] for s, _ in spans] or ht['points'] != len(spans):
        return [f'{ht["points"]} points against the rule\'s {len(spans)}']
    for k, v in hw.items():
        if isinstance(v, list) and len(v) == hw['points'] and ht.get(k) != [v[s] for s, _ in spans]:
            bad.append(f'history {k} is not the whole file\'s at the points')
    if ht['bin_updates'] != [e - s for s, e in spans]:
        bad.append('bin_updates')
    if ht['bin_judged'] != [sum(hw['judged'][s:e]) for s, e in spans]:
        bad.append('bin_judged')
    if 'bin_error_db_min' in ht:
        bad.append('a stepped test carries error extremes')
    d = np.array(hw['drive_re']) + 1j * np.array(hw['drive_im'])
    many = [i for i, (s, e) in enumerate(spans) if e - s > 1]
    if any(ht['bin_drive_vpk_max'][i] is not None or ht['bin_drive_norm_max'][i] is not None
           for i, (s, e) in enumerate(spans) if e - s == 1):
        bad.append('a drive extreme where the point stands for one update')
    vmax = np.array([np.abs(d[s:e]).max(0) for s, e in spans])[many]
    nmax = np.array([np.sqrt((np.abs(d[s:e]) ** 2).sum(1)).max() for s, e in spans])[many]
    got_v = np.array([ht['bin_drive_vpk_max'][i] for i in many], float).reshape(vmax.shape)
    got_n = np.array([ht['bin_drive_norm_max'][i] for i in many], float)
    if not np.allclose(got_v, vmax, rtol=rel, atol=0):
        bad.append('bin_drive_vpk_max is not each span\'s largest drive')
    if not np.allclose(got_n, nmax, rtol=rel, atol=0):
        bad.append('bin_drive_norm_max is not each span\'s largest norm')
    tw, tt = whole['true_modes'], thin['true_modes']
    if tw['tracked']:
        F = np.array(tw['frequency_hz'], float)
        if tt['frequency_hz'] != [tw['frequency_hz'][s] for s, _ in spans]:
            bad.append('true_modes frequency_hz')
        for key, f in (('frequency_hz_min', np.min), ('frequency_hz_max', np.max)):
            want = [None if e - s == 1 else [float(x) for x in f(F[s:e], 0)] for s, e in spans]
            if tt[key] != want:
                bad.append(f'true_modes {key}')
    for k in set(whole) | set(thin):
        if k in ('history', 'true_modes', 'options'):
            continue
        if whole.get(k) != thin.get(k):
            bad.append(f'{k} differs')
    if {**whole['options'], 'whole_history': None} != {**thin['options'], 'whole_history': None}:
        bad.append('options differ beyond whole_history')
    return bad


def broken_thinnings(thin):
    """Copies of a thinned run file built to break the thinning check, each with what was broken."""
    import copy
    h = thin['history']
    out = []
    a = copy.deepcopy(thin)
    k = h['judged'].index(True)
    for key, v in a['history'].items():
        if isinstance(v, list) and len(v) == h['points']:
            del v[k]
    a['history']['points'] -= 1
    out.append(('a judged update dropped', a))
    spans = [i for i, n in enumerate(h['bin_updates']) if n > 1]
    b = copy.deepcopy(thin)
    for i in spans:
        b['history']['bin_drive_vpk_max'][i] = [n9(x * 0.999) for x in h['bin_drive_vpk_max'][i]]
    out.append(('each span\'s largest drive 1e-3 low', b))
    b = copy.deepcopy(thin)
    k = h['bin_updates'].index(1)
    b['history']['bin_drive_vpk_max'][k] = [n9(abs(complex(x, y))) for x, y in zip(h['drive_re'][k], h['drive_im'][k])]
    b['history']['bin_drive_norm_max'][k] = 0.0
    out.append(('a drive extreme on a point standing for one update', b))
    if thin['true_modes']['tracked']:
        c = copy.deepcopy(thin)
        for i in spans:
            c['true_modes']['frequency_hz_max'][i] = [n9(x * (1 - 1e-6)) for x in c['true_modes']['frequency_hz_max'][i]]
        out.append(('each span\'s mode maximum 1e-6 low', c))
    return out


@unittest.skipUnless(RUN and PLAY and PROBLEMS, 'set MSNB_RUN, OBS_PLAY and MSNB_PROBLEMS')
class MsnbRun(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.mkdtemp(prefix='msnb_check_')
        dwell = {'type': 'dwell', 'freq_hz': 150.0, 'dwell_s': 6.0}
        rec = case(REALISTIC, dwell, 10.0, protective='widened')
        jobs = {
            'rec_a': (rec, ['--record', '--whole-history']),
            'rec_b': (rec, ['--record', '--whole-history']),
            'rec_seed': (dict(rec, identification_seed=7), ['--record', '--whole-history']),
            'thin_dwell': (rec, ['--record']),
            'ideal': (case(LINEAR, {'type': 'dwell', 'freq_hz': 150.0, 'dwell_s': 5.0}, 10.0), ['--ideal-frf']),
            'far': (case(LINEAR, {'type': 'dwell', 'freq_hz': 150.0, 'dwell_s': 5.0}, 1e5), []),
            'down': (case(LINEAR, {'type': 'sweep', 'band_hz': [200.0, 400.0], 'rate_oct_per_min': 4.0,
                                   'direction': 'down'}, 1.0), []),
            'big': (case(BIG, dwell, 10.0), []),
        }
        lin = {'type': 'dwell', 'freq_hz': 150.0, 'dwell_s': 5.0}
        amp = 10.0 * 30.0 / ideal_peak(LINEAR, lin, 10.0)            # the ideal drive peaks near 30 V
        cls.gain = float(f'{ideal_peak(LINEAR, lin, amp):.9g}')
        cls.ceiling = float(f'{10 ** (6 / 20):.9g}')
        jobs['gain'] = (case(LINEAR, lin, amp, drives={'gain': cls.gain, 'ceiling_vpk': cls.ceiling}), ['--ideal-frf'])
        jobs['no_gain'] = (case(LINEAR, lin, amp), ['--ideal-frf'])
        step = case(REALISTIC, {'type': 'stepped', 'band_hz': [100.0, 200.0], 'steps_per_octave': 6, 'step_s': 0.5},
                    10.0, protective='widened')
        jobs['step_whole'] = (step, ['--whole-history'])
        jobs['step_thin'] = (step, [])
        with cf.ThreadPoolExecutor(4) as ex:
            futs = {k: ex.submit(run, cls.tmp, k, c, *fl) for k, (c, fl) in jobs.items()}
            cls.res = {k: f.result() for k, f in futs.items()}

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def out(self, k):
        code, out, err = self.res[k]
        self.assertIn(code, (0, 1, 3), err)
        return out

    def test_1_the_same_case_gives_the_same_bytes(self):
        self.assertEqual(same_bytes(self.out('rec_a'), self.out('rec_b')), [])
        self.assertNotEqual(same_bytes(self.out('rec_a'), self.out('rec_seed')), [], 'the check cannot fail')

    def test_2_the_run_file_agrees_with_the_player(self):
        out = self.out('rec_a')
        self.assertEqual(load(out)['outcome']['class'], 'completed')
        self.assertEqual(replay_matches(out), [])
        self.assertNotEqual(replay_matches(out, step='hold'), [], 'the check cannot fail')
        self.assertEqual(phasors_match(out, 150.0), [])
        # A run file whose controls read 0.086 dB high.
        self.assertNotEqual(phasors_match(out, 150.0, scale=1.01), [], 'the check cannot fail')
        self.assertEqual(modes_match(out, REALISTIC), [])
        self.assertNotEqual(modes_match(out, REALISTIC, amp_scale=2.0), [], 'the check cannot fail')

    def test_3_a_linear_structure_with_the_ideal_response_completes_a_dwell(self):
        self.assertEqual(completes_within_alarm(self.out('ideal')), [])
        self.assertEqual(load(self.out('ideal'))['identification']['source'], 'ideal_frf')
        self.assertNotEqual(completes_within_alarm(self.out('far')), [], 'the check cannot fail')

    def test_4_an_unreachable_reference_ends_as_the_engine_says(self):
        self.assertEqual(ends_as_the_engine_says(self.out('far')), [])
        self.assertNotEqual(ends_as_the_engine_says(self.out('ideal')), [], 'the check cannot fail')

    def test_5_refusals(self):
        # Since Hertzforge#465 (4fd2a5c9) the engine takes 24 controls and 12 drives, so the
        # set's largest rig is run, not refused (exit 3 is kept for a rig over the engine's size;
        # the set holds none).
        code, out, err = self.res['big']
        self.assertNotEqual(code, 3, err)
        self.assertNotEqual(code, 2, err)
        o = load(out)['outcome']
        self.assertNotEqual(o['class'], 'refused', o)
        base = case(LINEAR, {'type': 'dwell', 'freq_hz': 150.0, 'dwell_s': 1.0}, 1.0)
        for name, c in [('unknown_key', dict(base, colour='red')),
                        ('unknown_param', dict(base, law={'id': 'matrix', 'params': {'no_such_key': 1}})),
                        ('short_reference', case(LINEAR, {'type': 'dwell', 'freq_hz': 150.0, 'dwell_s': 1.0}, 1.0,
                                                 band=(160.0, 300.0)))]:
            code, out, err = run(self.tmp, name, c)
            self.assertEqual(code, 2, f'{name}: {err}')
            self.assertFalse(os.path.exists(out), name)
        # The refusal check fails on a case the runner takes.
        code, _, err = run(self.tmp, 'taken', base)
        self.assertNotEqual(code, 2, err)

    def test_7_a_drive_gain_meets_a_reference_over_the_converters_range(self):
        self.assertGreater(self.gain, 10.0)
        self.assertEqual(gain_met(self.out('gain'), self.gain, self.ceiling), [])
        self.assertNotEqual(gain_met(self.out('no_gain'), self.gain, self.ceiling), [], 'the check cannot fail')

    def test_8_a_stepped_sine_and_a_dwell_are_thinned_as_stated(self):
        for whole, thin in (('rec_a', 'thin_dwell'), ('step_whole', 'step_thin')):
            w, t = load(self.out(whole)), load(self.out(thin))
            self.assertGreater(sum(w['history']['judged']), 10, whole)
            self.assertLess(t['history']['points'], w['history']['points'], f'{thin} kept every update')
            self.assertEqual(thinned_as_stated(w, t), [], thin)
            broken = broken_thinnings(t)
            self.assertEqual(len(broken), 4 if t['true_modes']['tracked'] else 3)
            for what, b in broken:
                self.assertNotEqual(thinned_as_stated(w, b), [], f'{thin}, {what}: the check cannot fail')

    def test_6_a_down_sweep_starts_at_the_top_and_falls(self):
        r = load(self.out('down'))
        f = np.array(r['history']['frequency_hz'])
        self.assertEqual(r['test']['start_hz'], 400.0)
        self.assertAlmostEqual(f[0], 400.0, places=6)
        self.assertTrue(np.all(np.diff(f) <= 0.0), 'the frequency rose somewhere')
        self.assertLess(f[-1], 400.0)


if __name__ == '__main__':
    if sys.argv[1:2] == ['case']:
        # Write one case as the checks build them (the timing runs use this):
        #   check_msnb_run.py case <problem id> '<test json>' <amplitude m/s^2> <out.json> ['<extra json>']
        pid, test, amp, path = sys.argv[2], json.loads(sys.argv[3]), float(sys.argv[4]), sys.argv[5]
        extra = json.loads(sys.argv[6]) if len(sys.argv) > 6 else {}
        with open(path, 'w') as fh:
            json.dump(case(pid, test, amp, **extra), fh)
    else:
        unittest.main()
