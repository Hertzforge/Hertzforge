"""Each elastic mode's modal displacement RMS at full level, for `softening.json`.

The softening's reference is the linear plant's response to the drive of the
`n_ba_r65` run, over the last frames scored. So at full level every elastic mode
sits half the softening fraction low, and at the system identification's level the
plant is linear.

    python examples/rc_plate_band_average/run_study.py OUT_DIR n_ba_r65 --keep
    python examples/rc_plate_band_average/make_softening.py OUT_DIR [--write]

It prints the ratio to the values now in `softening.json` and replaces them only
with `--write`.
"""

import json
import sys
from pathlib import Path

import numpy as np
from run_study import FRAME, HERE, HOP, SAMPLE_RATE, SCORED_FRAMES, load_models, read_stream
from scipy import signal

FRACTION = 0.16
SMOOTHING = 0.25
NOISE_ASD = 1e-5


def main() -> None:
    out = Path(sys.argv[1])
    plant, _, drives, controls = load_models()
    data = read_stream(out, 'n_ba_r65', len(controls) + len(drives))
    x = np.asarray(data[len(controls):, -(SCORED_FRAMES * HOP + HOP):], dtype=float)
    f, sxx = signal.csd(x[:, None, :], x[None, :, :], SAMPLE_RATE, window='hann', nperseg=FRAME, noverlap=HOP)
    sxx = np.moveaxis(sxx, -1, 0)
    A, B = plant['A'], plant['B']
    q = np.arange(0, A.shape[0], 2)
    q = q[np.sqrt(-A[q + 1, q]) / (2 * np.pi) > 10.0]
    s = 2j * np.pi * f
    hq = np.linalg.solve(s[:, None, None] * np.eye(A.shape[0]) - A, np.broadcast_to(B, (f.size,) + B.shape))[:, q]
    sqq = np.real(np.einsum('fmd,fde,fme->mf', hq, sxx, hq.conj()))
    rms = np.sqrt(sqq.sum(1) * (f[1] - f[0]))
    old = np.array(json.loads((HERE / 'softening.json').read_text())['reference_rms'])
    print(f'ratio to softening.json: min {(rms / old).min():.3f}, median {np.median(rms / old):.3f}, max {(rms / old).max():.3f}')
    if '--write' in sys.argv:
        (HERE / 'softening.json').write_text(json.dumps(
            {'reference_rms': rms.tolist(), 'fraction': FRACTION, 'smoothing': SMOOTHING,
             'noise_asd_g2_hz': NOISE_ASD}, indent=1))


if __name__ == '__main__':
    main()
