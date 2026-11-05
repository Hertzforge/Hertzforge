"""Band-average control on the RC plate, run in Hertzforge.

Takes the plant and the specification from `make_rc_plate.py` and
`make_barc.py`, runs each named test on Hertzforge's multiple-output Random
service (`crates/hertzforge-mimo-random`) with the plant on the simulator, and
scores it. Runs are named `n_<law>` on the linear plant and `nl8n_<law>` on the
plant whose modes soften 8 % at full level, both with white response noise of
1e-5 g^2/Hz.

    cargo build --release -p hertzforge-mimo-random --example mimo_study
    python examples/rc_plate_band_average/run_study.py OUT_DIR [run ...]

With no names it runs every one in `RUNS`. Each stream is about 1 GB, and is
deleted once it is scored unless `--keep` is given. Scores are appended to
`OUT_DIR/scores.jsonl`.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

import numpy as np
from scipy import signal

HERE = Path(__file__).parent
ROOT = HERE.parents[1]
SAMPLE_RATE = 16384
FRAME = 16384
HOP = FRAME // 2
SCORED_FRAMES = 117
LEVELS = (-12, -9, -6, -3, 0)
STEP_S = 30.0
HOLD_S = 90.0
BA = {}
R70 = BA | {'drive_reduction': 0.70}
CROSS = BA | {'cross_terms': 'reference'}
TFU = {'use_tf_update': True}
#: law name: (kind, parameters, update the FRF during control)
LAWS = {
    'pinv': ('pseudoinverse', {}, False),
    'ba_knee': ('band_average', BA, False),
    'ba_knee_tfu': ('band_average', BA | TFU, True),
    'ba_r65': ('band_average', BA | {'drive_reduction': 0.65}, False),
    'ba_r70_nogc': ('band_average', R70 | {'gain_correction': False}, False),
    'ba_r70': ('band_average', R70, False),
    'ba_r70_tfu': ('band_average', R70 | TFU, True),
    'ba_r70_tfu_coh': ('band_average', R70 | TFU | {'min_coherence': 0.8}, True),
    'ba_r70_tfu_hold': ('band_average', R70 | TFU | {'hold_capped_bands': True}, True),
    'ba_cross': ('band_average', CROSS, False),
    'ba_cross_tfu': ('band_average', CROSS | TFU, True),
}
# The service runs a law call per segment. Two blocks of 16384 samples, 2 s, hold
# three 50 % overlapped frames, and Rattlesnake's test runs called the law about
# every 2 s. An exponential average over 20/3 loops has the variance of the
# 20-frame rolling CPSD Rattlesnake uses.
FRAMES_PER_LOOP = 2
MEMORY_LOOPS = 20 / 3
# The service's identification drive covers the band only. This RMS gives the PSD
# of 4 lbf RMS flat to Nyquist, which is what Rattlesnake plays. The FRF's noise
# sets how far a plain pseudoinverse reaches into the plant's weak directions.
SYSID_RMS = 4.0 * np.sqrt(3980 / (SAMPLE_RATE / 2))
SEED_TAG = ''
LAW_FILE = ''
RUNS = [f'n_{law}' for law in ('pinv', 'ba_knee', 'ba_r70', 'ba_r70_tfu', 'ba_cross')] + [
    f'nl8n_{law}' for law in LAWS]


def load_models():
    plant = np.load(HERE / 'rc_plate_state_space.npz', allow_pickle=True)
    spec = np.load(HERE / 'barc_response_spec.npz', allow_pickle=True)
    inputs = [str(dof) for dof in plant['input_dof']]
    names = [str(dof) for dof in plant['channel_dof']]
    drives = [names.index(dof) for dof in inputs]
    controls = [k for k in range(len(names)) if k not in set(drives)]
    return plant, spec, drives, controls


def write_shared(out: Path) -> None:
    """The plant and the specification as raw arrays, once."""
    plant, spec, _, controls = load_models()
    path = out / 'plant.f64'
    if not path.exists():
        with open(path, 'wb') as f:
            f.writelines(np.asarray(plant[key], dtype='<f8').tobytes() for key in 'ABCD')
    path = out / 'specification.c128'
    if not path.exists():
        lines = FRAME // 2 + 1
        full = np.zeros((lines, len(controls), len(controls)), dtype='<c16')
        k = np.round(spec['f'] / (SAMPLE_RATE / FRAME)).astype(int)
        full[k] = spec['cpsd']
        full.tofile(path)


def stable_seed(name: str) -> int:
    """A seed from a run's name, the same on every machine."""
    value = 0
    for ch in name.encode():
        value = (value * 131 + ch) % (2**31 - 1)
    return value


def write_setup(out: Path, run: str) -> Path:
    """The study driver's setup file for one run."""
    plant, _, drives, controls = load_models()
    prefix, law = run.split('_', 1)
    kind, parameters, update = LAWS[law]
    softening = json.loads((HERE / 'softening.json').read_text())
    soft = None
    if prefix == 'nl8n':
        q = np.arange(0, plant['A'].shape[0], 2)
        q = q[np.sqrt(-plant['A'][q + 1, q]) / (2 * np.pi) > 10.0]
        soft = {'displacement_states': q.tolist(), 'reference_rms': softening['reference_rms'],
                'fraction': softening['fraction'], 'smoothing': softening['smoothing'], 'read_samples': HOP,
                'log': str((out / f'{run}_softening.csv').resolve())}
        levels, full = list(LEVELS), HOLD_S
    else:
        levels, full = [0], 90.0
    law_entry = {'kind': kind, 'params': parameters}
    if LAW_FILE and kind == 'band_average':
        law_entry = {'kind': 'file', 'path': str(Path(LAW_FILE).resolve()), 'params': parameters}
    setup = {
        'sample_rate': SAMPLE_RATE, 'nfft': FRAME, 'band_lo_hz': 20.0, 'band_hi_hz': 4000.0,
        'frames_per_loop': FRAMES_PER_LOOP, 'memory_loops': MEMORY_LOOPS,
        'noise_frames': 10, 'sysid_frames': 50, 'sysid_rms': SYSID_RMS,
        'level_ramp_s': 1.0, 'levels': levels, 'dwell_s': STEP_S, 'full_level_s': full,
        'update_frf': update, 'seed': stable_seed(run + SEED_TAG),
        'plant': {'file': 'plant.f64', 'states': int(plant['A'].shape[0]), 'inputs': len(drives),
                  'outputs': int(plant['C'].shape[0]), 'softening': soft,
                  'noise_rms': float(np.sqrt(softening['noise_asd_g2_hz'] * SAMPLE_RATE / 2)),
                  'stream': str((out / f'{run}.f32').resolve())},
        'control_outputs': controls, 'drive_outputs': drives,
        'specification': 'specification.c128',
        'law': law_entry,
    }
    path = out / f'{run}.json'
    path.write_text(json.dumps(setup, indent=1))
    return path


def simulator() -> str:
    target = os.environ.get('CARGO_TARGET_DIR', str(ROOT / 'target'))
    return str(Path(target) / 'release' / 'examples' / 'mimo_study')


def softened_frf(plant, f_hz, f, q):
    v = q + 1
    w0 = np.sqrt(-plant['A'][v, q])
    zeta = -plant['A'][v, v] / (2 * w0)
    A, C = plant['A'].copy(), plant['C'].copy()
    w = 2 * np.pi * f_hz
    ratio = w / w0
    A[v, q] = -w**2
    A[v, v] = -2 * zeta * w
    C[:, q] *= ratio**2
    C[:, v] *= ratio
    s = 2j * np.pi * f
    x = np.linalg.solve(s[:, None, None] * np.eye(A.shape[0]) - A,
                        np.broadcast_to(plant['B'], (f.size,) + plant['B'].shape))
    return np.einsum('cs,fsd->fcd', C, x) + plant['D']


def cpsd_sum(x, n):
    """The unscaled Hann-frame CPSD sum, (f, c, c), E[X conj(Y)]."""
    window, hop, g = np.hanning(n + 1)[:-1], n // 2, 0
    for k in range((x.shape[1] - n) // hop + 1):
        spectra = np.fft.rfft(x[:, k * hop:k * hop + n] * window, axis=1).T
        g = g + np.einsum('fi,fj->fij', spectra, spectra.conj())
    return g


def narrowband(g, ss, iu):
    """|rho_meas - rho_spec| per line and pair, and the phase error where the spec's coherence exceeds 0.5."""
    def rho(c):
        a = np.sqrt(np.real(np.einsum('fii->fi', c)))
        return (c / (a[:, :, None] * a[:, None, :]))[:, iu[0], iu[1]]
    rm, rs = rho(g), rho(ss)
    err = np.abs(rm - rs)
    g2 = np.abs(np.abs(rm) ** 2 - np.abs(rs) ** 2)
    sig = np.abs(rs) ** 2 > 0.5
    phi = np.degrees(np.abs(np.angle(rm[sig] * rs[sig].conj())))
    return err.mean(), np.median(err), g2.mean(), np.median(phi), np.percentile(phi, 90)


def read_stream(out: Path, run: str, channels: int):
    """The stream as channels by time: controls, then the drives as played."""
    samples = np.memmap(out / f'{run}.f32', dtype='<f4', mode='r').reshape(-1, channels)
    return np.asarray(samples).T


def score(out: Path, run: str) -> dict:
    """The study's numbers for one finished run."""
    plant, _, drives, controls = load_models()
    summary = json.loads((out / f'{run}_summary.json').read_text())
    data = read_stream(out, run, len(controls) + len(drives))
    plant_controls = controls
    controls = list(range(len(plant_controls)))
    drives = list(range(len(plant_controls), len(plant_controls) + len(drives)))
    linear = run.startswith('n_')
    f = np.arange(FRAME // 2 + 1) * SAMPLE_RATE / FRAME
    ss = np.fromfile(out / 'specification.c128', dtype='<c16').reshape(f.size, len(controls), len(controls))
    spec = np.real(np.einsum('fii->if', ss))
    band = (f >= 20) & (f <= 4000)
    centers = 1000 * 2.0 ** (np.arange(-60, 13) / 6)
    centers = centers[(centers >= 20 * 2**(1 / 12)) & (centers <= 4000 / 2**(1 / 12))]
    masks = [(f >= c / 2**(1 / 12)) & (f < c * 2**(1 / 12)) for c in centers]

    def band_err(p, s):
        return np.array([10 * np.log10(p[:, m].mean(1) / s[:, m].mean(1)) for m in masks]).T

    n = data.shape[1]
    t = np.asarray(data[:, max(n - (SCORED_FRAMES * HOP + HOP), 0):], dtype=float)
    _, p = signal.welch(t, SAMPLE_RATE, window='hann', nperseg=FRAME, noverlap=HOP)
    meas = p[controls]
    be = band_err(meas, spec)
    rms = 10 * np.log10(meas[:, band].sum(1) / spec[:, band].sum(1))
    out6 = (np.abs(10 * np.log10(meas[:, band] / spec[:, band])) > 6).mean() * 100
    drms = np.sqrt(p[drives][:, band].sum(1))
    g = cpsd_sum(t[controls], FRAME)
    iu = np.triu_indices(len(controls), 1)
    nb = narrowband(g[band], ss[band], iu)

    q = np.arange(0, plant['A'].shape[0], 2)
    q = q[np.sqrt(-plant['A'][q + 1, q]) / (2 * np.pi) > 10.0]
    f0 = np.sqrt(-plant['A'][q + 1, q]) / (2 * np.pi)
    if linear:
        f_full = f0
    else:
        log = np.loadtxt(out / f'{run}_softening.csv', delimiter=',')
        window = log[:, 0] >= log[-1, 0] - (SCORED_FRAMES * HOP + HOP) / SAMPLE_RATE
        f_full = log[window, 1:].mean(0)
    shift = 100 * (f_full / f0 - 1)
    _, sxx = signal.csd(t[drives][:, None], t[drives][None], SAMPLE_RATE, window='hann', nperseg=FRAME, noverlap=HOP)
    sxx = np.moveaxis(sxx, -1, 0).conj()
    hs = softened_frf(plant, f_full, f, q)[:, plant_controls]
    pred = np.real(np.einsum('fcd,fde,fce->cf', hs, sxx, hs.conj()))
    fid = np.array([10 * np.log10(meas[:, m].mean(1) / pred[:, m].mean(1)) for m in masks])

    # The control loops are the end of the stream, one segment each.
    engaged = n - int(summary['loop_count'] * summary['segment_samples'])
    trace = []
    for start in range(engaged, n - (10 * HOP + HOP) + 1, 5 * SAMPLE_RATE):
        seg = np.asarray(data[:, start:start + 10 * HOP + HOP], dtype=float)[controls]
        tc = (start - engaged + (10 * HOP + HOP) / 2) / SAMPLE_RATE
        level = 0 if linear else LEVELS[min(int(tc // STEP_S), len(LEVELS) - 1)]
        _, pw = signal.welch(seg, SAMPLE_RATE, window='hann', nperseg=FRAME, noverlap=HOP)
        trace.append((tc, level, np.abs(band_err(pw, spec * 10 ** (level / 10))).mean()))
    trace = np.array(trace)
    ends, at_full = [], []
    if not linear:
        for k in range(len(LEVELS) - 1):
            sel = (trace[:, 0] + 5.5 / 2 <= (k + 1) * STEP_S) & (trace[:, 0] - 5.5 / 2 >= k * STEP_S)
            ends.append(float(trace[sel, 2][-1]) if sel.any() else float('nan'))
        full = trace[trace[:, 0] - 5.5 / 2 >= 4 * STEP_S]
        at_full = [float(full[np.argmin(np.abs(full[:, 0] - (4 * STEP_S + s)))][2]) for s in (5, 30, 85)]
    seconds = np.array([row['law_ms'] / 1e3 for row in summary['loops']])
    row = {'run': run, 'state': summary['state'], 'simulated_s': (n - engaged) / SAMPLE_RATE,
           'band_db': float(np.nanmean(np.abs(be))),
           'rms_db': [float(rms.min()), float(rms.max())], 'lines_out6_pct': float(out6),
           'drive_lbf2': float(np.sum(drms**2)), 'rho_err_mean': float(nb[0]), 'rho_err_median': float(nb[1]),
           'g2_err_mean': float(nb[2]), 'phase_median_deg': float(nb[3]), 'phase_p90_deg': float(nb[4]),
           'shift_pct': [float(shift.min()), float(shift.max()), float(shift.mean())],
           'fidelity_median_db': float(np.median(fid)), 'fidelity_range_db': [float(fid.min()), float(fid.max())],
           'dwell_end_db': ends, 'at_full_db': at_full, 'law_calls': int(summary['loop_count']),
           'law_stats': summary['law_stats'], 'capped_lines': int(sum(r['capped'] for r in summary['loops'])),
           'repaired_lines': int(sum(r['repaired'] for r in summary['loops'])),
           'first_solve_s': float(summary['first_solve_s']),
           'law_median_s': float(np.median(seconds)) if seconds.size else None,
           'wall_s': float(summary['wall_s'])}
    np.savez(out / f'{run}_trace.npz', trace=trace, be=be, shift=shift, fid=fid, drive_rms=drms,
             psd_channels=meas, frequency=f, law_seconds=seconds)
    with open(out / 'scores.jsonl', 'a') as stream:
        stream.write(json.dumps(row) + '\n')
    return row


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('out', type=Path)
    parser.add_argument('runs', nargs='*')
    parser.add_argument('--keep', action='store_true', help='keep each stream after scoring it')
    parser.add_argument('--repeat', default='', help='tag added to the seed, for a repeat of a run')
    parser.add_argument('--law-file', default='', help='run the band-average law from this law file')
    args = parser.parse_args(argv)
    global SEED_TAG, LAW_FILE
    SEED_TAG = args.repeat
    LAW_FILE = args.law_file
    args.out.mkdir(parents=True, exist_ok=True)
    write_shared(args.out)
    for run in args.runs or RUNS:
        setup = write_setup(args.out, run)
        print(f'== {run}', flush=True)
        with open(args.out / f'{run}.log', 'w') as log:
            subprocess.run([simulator(), str(setup), str(args.out / f'{run}_summary.json')], check=True, stderr=log)
        row = score(args.out, run)
        print(f"  {row['state']}: band {row['band_db']:.2f} dB, RMS {row['rms_db'][0]:+.2f}..{row['rms_db'][1]:+.2f}, "
              f"{row['lines_out6_pct']:.1f} % lines out, drive {row['drive_lbf2']:.3g} lbf^2, "
              f"rho {row['rho_err_mean']:.3f}, fidelity {row['fidelity_median_db']:+.3f} dB, "
              f"first solve {row['first_solve_s']:.1f} s, {row['law_calls']} loops, "
              f"{row['capped_lines']} capped, {row['repaired_lines']} repaired, {row['wall_s']:.0f} s",
              flush=True)
        if not args.keep:
            (args.out / f'{run}.f32').unlink()
    return 0


if __name__ == '__main__':
    sys.exit(main())
