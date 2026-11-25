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
   exit 2 and no folder, and records the engine's refusal of a rig over its size with exit 3.
6. A down sweep starts at the top of the band and falls.

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


@unittest.skipUnless(RUN and PLAY and PROBLEMS, 'set MSNB_RUN, OBS_PLAY and MSNB_PROBLEMS')
class MsnbRun(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.mkdtemp(prefix='msnb_check_')
        dwell = {'type': 'dwell', 'freq_hz': 150.0, 'dwell_s': 6.0}
        rec = case(REALISTIC, dwell, 10.0, protective='widened')
        jobs = {
            'rec_a': (rec, ['--record']),
            'rec_b': (rec, ['--record']),
            'rec_seed': (dict(rec, identification_seed=7), ['--record']),
            'ideal': (case(LINEAR, {'type': 'dwell', 'freq_hz': 150.0, 'dwell_s': 5.0}, 10.0), ['--ideal-frf']),
            'far': (case(LINEAR, {'type': 'dwell', 'freq_hz': 150.0, 'dwell_s': 5.0}, 1e5), []),
            'down': (case(LINEAR, {'type': 'sweep', 'band_hz': [200.0, 400.0], 'rate_oct_per_min': 4.0,
                                   'direction': 'down'}, 1.0), []),
            'big': (case(BIG, dwell, 10.0), []),
        }
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
        code, out, err = self.res['big']
        self.assertEqual(code, 3, err)
        o = load(out)['outcome']
        self.assertEqual(o['class'], 'refused')
        self.assertIn('controls_out_of_range', o['message'])
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
