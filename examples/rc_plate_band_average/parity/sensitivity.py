"""How much of the Rust-Python difference the Python law has against itself.

Runs the same closed loop as make_reference.py with the identified FRF disturbed
at rounding level (each entry times 1 + 4e-16 x a seeded normal, the size of the
rounding the two ports' arithmetic cannot share), and compares each call's drive
with the saved reference. The PSD re-fit's iterations amplify that disturbance,
and the gain correction feeds it around the loop.

    BAND_AVERAGE_LAW=path/to/law.py python sensitivity.py OUT_DIR CASE [CASE ...]
"""

import contextlib
import io
import os
import sys

import make_reference as mr
import numpy as np


def compare(a, b):
    diff = np.sum(np.abs(a - b) ** 2, axis=(1, 2))
    norm = np.sum(np.abs(b) ** 2, axis=(1, 2))
    rel = np.sqrt(np.where(norm > 0, diff / np.where(norm > 0, norm, 1.0), 0.0))
    return rel.max(), int(rel.argmax())


def main():
    args = sys.argv[1:]
    out = args.pop(0)
    law_module = mr.load_law()
    n_lines, n_ctrl, n_drive = mr.N_LINES, 48, 12
    read = lambda name, shape, dtype=complex: np.fromfile(os.path.join(out, name), dtype=dtype).reshape(shape)
    h_id = read('h_id.bin', (n_lines, n_ctrl, n_drive))
    h_true = read('h_true.bin', (n_lines, n_ctrl, n_drive))
    spec = read('spec.bin', (n_lines, n_ctrl, n_ctrl))
    sysid = read('sysid.bin', (n_lines, n_ctrl, n_ctrl))
    coherence = read('coherence.bin', (n_lines, n_ctrl), float)
    response_jitter = read('response_jitter.bin', (mr.N_CALLS, n_lines), float)
    output_jitter = read('output_jitter.bin', (mr.N_CALLS, n_lines), float)
    estimates = [None] + [read(f'estimate_{k}.bin', (n_lines, n_ctrl, n_drive)) for k in range(1, mr.N_CALLS)]
    disturbed = h_id * (1.0 + 4e-16 * np.random.default_rng(1).standard_normal(h_id.shape))

    for case in [int(a) for a in args]:
        case_dir = os.path.join(out, f'case_{case:02d}')
        with open(os.path.join(case_dir, 'params.txt')) as f:
            params = f.read()
        with open(os.path.join(case_dir, 'fail.txt')) as f:
            fail = int(f.read())
        with contextlib.redirect_stdout(io.StringIO()):
            law = law_module.band_average(specification=spec, extra_parameters=params,
                                          transfer_function=disturbed, sysid_response_cpsd=sysid)
            response, output = None, None
            row = []
            for call in range(mr.N_CALLS):
                drive = law.control(transfer_function=disturbed if call == 0 else estimates[call],
                                    multiple_coherence=coherence, frames=mr.FRAMES[call],
                                    total_frames=mr.TOTAL_FRAMES, last_response_cpsd=response,
                                    last_output_cpsd=output)
                reference = read(os.path.join(f'case_{case:02d}', f'drive_{call}.bin'), (n_lines, n_drive, n_drive))
                row.append(compare(drive, reference))
                if call + 1 < mr.N_CALLS:
                    response = (h_true @ drive @ np.conj(np.swapaxes(h_true, 1, 2)))
                    response = response * response_jitter[call][:, None, None]
                    if fail >= 0 and call + 1 in (3, 4):
                        response[:, fail, :] *= mr.FAIL_AMPLITUDE
                        response[:, :, fail] *= mr.FAIL_AMPLITUDE
                    output = drive * output_jitter[call][:, None, None]
        print(f'case {case:02d}: python against itself, largest {max(r[0] for r in row):.2e} at line '
              f'{max(row)[1]}; per call ' + ' '.join(f'{r[0]:.1e}' for r in row), flush=True)


if __name__ == '__main__':
    main()
