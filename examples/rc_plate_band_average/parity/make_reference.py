"""Writes the reference for the band-average law's Rust port: drives the Python
law around a simulated closed loop on the RC plate and saves what the Rust side
needs to run the same loop and compare, call by call.

    BAND_AVERAGE_LAW=path/to/law.py python make_reference.py OUT_DIR [--cases 0,3,...]

`BAND_AVERAGE_LAW` names the Python law the port follows; it is not part of this
repository. The plant and the specification are the study's, from
`make_rc_plate.py` and `make_barc.py`.

Then, from the repository root:

    cargo run --release -p hertzforge-law-band-average-math --example parity -- OUT_DIR

The plant is the exact FRF of the study's state-space model. The law identifies
on it with 1 % seeded multiplicative noise; the simulated article is the same
model with every pole's frequency lowered 1 %, so the gain correction and the FRF
updates have a real model error to correct. All arrays are raw little-endian
(complex128 or float64), row-major.
"""

import contextlib
import importlib.util
import io
import os
import sys
import time

import numpy as np

STUDY = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LAW = os.environ.get('BAND_AVERAGE_LAW', '')

SEED = 20261105
N_LINES = 8193
N_CALLS = 7
TOTAL_FRAMES = 50
# The third call's window is not full, and the fifth call's estimate has a NaN in
# it, so the FRF update's two guards are both exercised.
FRAMES = [50, 50, 30, 50, 50, 50, 50]
NAN_ESTIMATE_CALL = 4
FAIL_AMPLITUDE = 0.03    # -30 dB on one gauge, calls 3 and 4, for the channel check

BASE = 'df:1.0\ndrive_rms_limit:none'
RMS = 'df:1.0\ndrive_rms_limit:{rms}'
# (parameters, channel that fails or -1). The first six are the comparison asked
# for; the rest reach the options those do not.
CASES = [
    (BASE, -1),
    (BASE + '\ndrive_reduction:0.70', -1),
    (BASE + '\ndrive_reduction:0.70\ngain_correction:False', -1),
    (BASE + '\ndrive_reduction:0.70\nuse_tf_update:True', -1),
    (BASE + '\ncontrol_to_cross_terms:True', -1),
    (BASE + '\ncontrol_to_cross_terms:True\nuse_tf_update:True', -1),
    (BASE + '\nkappa_rule:knee', -1),
    (BASE + '\nkappa_rule:lcurve', -1),
    (BASE + '\nkappa_rule:knee\ndrive_reduction:0.5', -1),
    (RMS, -1),
    (RMS + '\nkappa_rule:knee', -1),
    (RMS + '\nuse_tf_update:True', -1),
    (BASE + '\ncondition_number:300', -1),
    (BASE + '\nregularization:none\nconstrained_solve:False', -1),
    (BASE + '\nuse_buzz:True', -1),
    (BASE + '\nlimit_channel_indices:1,2,30', -1),
    (BASE + '\nlimit_channel_indices:1,2,30\nuse_tf_update:True', -1),
    (BASE + '\nuse_tf_update:True\nmin_coherence:0.8\nhold_capped_bands:True', -1),
    (BASE + '\nuse_tf_update:True\ntf_update_blend:0.1\nn_e_seed:10', -1),
    (BASE + '\nuse_tf_update:True', 5),
    (BASE + '\nuse_buzz:True\nuse_tf_update:True\nmin_coherence:0.8', -1),
]


def load_law():
    spec = importlib.util.spec_from_file_location('band_average_law', LAW)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def exact_frfs():
    """The identified plant's exact FRF, and the softened article's, control rows only."""
    s = np.load(os.path.join(STUDY, 'rc_plate_state_space.npz'))
    A, B, C, D = s['A'], s['B'], s['C'], s['D']
    n_echo = s['input_dof'].size
    assert list(s['channel_dof'][:n_echo]) == list(s['input_dof'])   # drive echoes come first
    assert set(s['channel_unit'][:n_echo]) == {'lbf'} and set(s['channel_unit'][n_echo:]) == {'g'}
    C, D = C[n_echo:], D[n_echo:]
    w = 2j * np.pi * np.arange(N_LINES, dtype=float)
    poles, V = np.linalg.eig(A)
    left, right = C @ V, np.linalg.solve(V, B)
    softened = poles.real + 1j * poles.imag * 0.99

    def frf(p):
        return (left[None] / (w[:, None, None] - p[None, None, :])) @ right + D
    return frf(poles), frf(softened)


def main():
    args = [a for a in sys.argv[1:] if not a.startswith('--')]
    if not args or not LAW:
        sys.exit('usage: BAND_AVERAGE_LAW=law.py python make_reference.py OUT_DIR [--cases ...]')
    out = args[0]
    os.makedirs(out, exist_ok=True)
    chosen = None
    for a in sys.argv[1:]:
        if a.startswith('--cases='):
            chosen = {int(x) for x in a.split('=')[1].split(',')}

    rng = np.random.default_rng(SEED)
    noise = lambda shape: (rng.standard_normal(shape) + 1j * rng.standard_normal(shape)) / np.sqrt(2)

    exact, h_true = exact_frfs()
    h_id = exact * (1.0 + 0.01 * noise(exact.shape))
    h_id.tofile(os.path.join(out, 'h_id.bin'))
    h_true.tofile(os.path.join(out, 'h_true.bin'))
    n_lines, n_ctrl, n_drive = h_id.shape

    d = np.load(os.path.join(STUDY, 'barc_response_spec.npz'))
    assert np.array_equal(d['f'], np.arange(4097.0))
    spec = np.zeros((n_lines, n_ctrl, n_ctrl), dtype=complex)
    spec[:4097] = d['cpsd']
    spec.tofile(os.path.join(out, 'spec.bin'))

    # The buzz test's response CPSD: unit uncorrelated drives through the identified plant.
    sysid = h_id @ np.conj(np.swapaxes(h_id, 1, 2))
    sysid.tofile(os.path.join(out, 'sysid.bin'))

    coherence = np.clip(0.97 + 0.02 * rng.standard_normal((n_lines, n_ctrl)), 0.0, 1.0)
    weak = rng.random((n_lines, n_ctrl)) < 0.15
    coherence[weak] = 0.3
    coherence[:, 7] = np.where(np.arange(n_lines) % 3 == 0, 0.95, 0.2)   # one row under half its band
    coherence.tofile(os.path.join(out, 'coherence.bin'))

    estimates = [None] * N_CALLS
    for call in range(1, N_CALLS):
        estimates[call] = h_true * (1.0 + 0.02 * noise(h_true.shape))
        if call == NAN_ESTIMATE_CALL:
            estimates[call][1000, 3, 4] = np.nan
        estimates[call].tofile(os.path.join(out, f'estimate_{call}.bin'))
    response_jitter = 1.0 + 0.01 * rng.standard_normal((N_CALLS, n_lines))
    output_jitter = 1.0 + 0.01 * rng.standard_normal((N_CALLS, n_lines))
    response_jitter.tofile(os.path.join(out, 'response_jitter.bin'))
    output_jitter.tofile(os.path.join(out, 'output_jitter.bin'))

    with open(os.path.join(out, 'manifest.txt'), 'w') as f:
        f.write(f'n_lines {n_lines}\nn_controls {n_ctrl}\nn_drives {n_drive}\nn_calls {N_CALLS}\n')
        f.write(f'total_frames {TOTAL_FRAMES}\nn_cases {len(CASES)}\nfail_amplitude {FAIL_AMPLITUDE!r}\n')
        f.write('frames ' + ' '.join(str(x) for x in FRAMES) + '\n')

    law_module = load_law()

    def run(params, fail, case_dir):
        os.makedirs(case_dir, exist_ok=True)
        with open(os.path.join(case_dir, 'params.txt'), 'w') as f:
            f.write(params)
        with open(os.path.join(case_dir, 'fail.txt'), 'w') as f:
            f.write(str(fail))
        stdout = io.StringIO()
        times = []
        rms = None
        with contextlib.redirect_stdout(stdout):
            t0 = time.perf_counter()
            law = law_module.band_average(specification=spec, extra_parameters=params,
                                          transfer_function=h_id, sysid_response_cpsd=sysid)
            construct = time.perf_counter() - t0
            drive, response, output = None, None, None
            for call in range(N_CALLS):
                t0 = time.perf_counter()
                drive = law.control(transfer_function=h_id if call == 0 else estimates[call],
                                    multiple_coherence=coherence, frames=FRAMES[call],
                                    total_frames=TOTAL_FRAMES, last_response_cpsd=response,
                                    last_output_cpsd=output)
                times.append(time.perf_counter() - t0)
                np.asarray(drive).tofile(os.path.join(case_dir, f'drive_{call}.bin'))
                if call == 0:
                    rms = np.sqrt(np.real(np.einsum('fii->i', drive)))
                if call + 1 < N_CALLS:
                    # what the next call measures: this call's drive, through the article
                    response = (h_true @ drive @ np.conj(np.swapaxes(h_true, 1, 2)))
                    response = response * response_jitter[call][:, None, None]
                    if fail >= 0 and call + 1 in (3, 4):
                        response[:, fail, :] *= FAIL_AMPLITUDE
                        response[:, :, fail] *= FAIL_AMPLITUDE
                    output = drive * output_jitter[call][:, None, None]
        kappa = law.band_kappa if law.band_kappa is not None else np.zeros(0)
        np.asarray(kappa, dtype=float).tofile(os.path.join(case_dir, 'band_kappa.bin'))
        with open(os.path.join(case_dir, 'log.txt'), 'w') as f:
            f.write(stdout.getvalue())
        with open(os.path.join(case_dir, 'meta.txt'), 'w') as f:
            f.write('times ' + ' '.join(f'{construct + t if i == 0 else t!r}' for i, t in enumerate(times)) + '\n')
            front = -1 if law._front is None else law._front_i
            n_front = -1 if law._front is None else len(law._front)
            f.write(f'front {front} {n_front}\n')
        print(f'{os.path.basename(case_dir)}  ' + ' '.join(f'{t:.2f}' for t in times)
              + f' s  bands {law.n_bands}  rms max {rms.max():.4g}', flush=True)
        return rms

    # The RMS-limit cases need a limit that binds: 70 % of the largest drive's RMS
    # without a limit, at the unconstrained knee.
    rms_path = os.path.join(out, 'rms_limit.txt')
    rms_limit = None
    if os.path.exists(rms_path):
        with open(rms_path) as f:
            rms_limit = float(f.read())
    for i, (params, fail) in enumerate(CASES):
        if chosen is not None and i not in chosen:
            continue
        if '{rms}' in params and rms_limit is None:
            raise SystemExit('run case 0 first')
        result = run(params.replace('{rms}', f'{rms_limit!r}'), fail, os.path.join(out, f'case_{i:02d}'))
        if i == 0:
            rms_limit = 0.7 * float(result.max())
            with open(rms_path, 'w') as f:
                f.write(repr(rms_limit))


if __name__ == '__main__':
    main()
