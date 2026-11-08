"""Write the native reference fixture the controller's reference crate reads.

`mimo_reference/three_channel/` is a reference converted by the analysis
service from a synthetic specification `.mat` file (MATLAB v5), so the
controller's test of it checks the two languages agree on the file. The
specification:

- three channels, `front`, `rear` and `top`, in g;
- lines every 5 Hz from 20 to 100 Hz on a 5 Hz grid of 41 lines (0 to
  200 Hz);
- autospectra 0.01, 0.02 and 0.04 g^2/Hz;
- `front` and `rear` with squared coherence 0.5, `rear` lagging `front` by
  30 degrees, written in the Bendat and Piersol form the file is assumed to
  use (the cross term at -30 degrees), so the controller holds +30;
- `front` and `top` with squared coherence 0.25 in phase, `rear` and `top`
  independent;
- warning lines at twice and abort lines at four times each autospectrum,
  with no upper warning line for `top` at 20 Hz.

Run from the repository root:

    python testdata/generate_mimo_reference.py
"""

from __future__ import annotations

import os
import pathlib
import sys
import tempfile

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
OUT = HERE / 'mimo_reference' / 'three_channel'

#: the grid and the recorded creation time, fixed so the binary, and with it
#: the content hash, regenerates identically
DF_HZ = 5.0
LINES = 41
CREATED = 1_793_750_400


def specification() -> dict[str, np.ndarray]:
    """The specification's variables in MATLAB's axis order (channels by
    channels by lines; channels by lines)."""
    f = np.arange(20, 105, 5)
    psd = np.array([0.01, 0.02, 0.04])
    coherence = np.array([[1.0, 0.5, 0.25], [0.5, 1.0, 0.0], [0.25, 0.0, 1.0]])
    # Bendat form: conj of E[X X^H]; rear lags front by 30 degrees
    phase = np.zeros((3, 3))
    phase[0, 1], phase[1, 0] = -30.0, 30.0
    one = (np.sqrt(np.outer(psd, psd) * coherence)
           * np.exp(1j * np.radians(phase)))
    cpsd = np.repeat(one[:, :, np.newaxis], len(f), axis=2)
    warning_upper = np.repeat((2 * psd)[:, np.newaxis], len(f), axis=1)
    warning_upper[2, 0] = np.nan
    return {
        'cpsd': cpsd,
        'f': f.astype(float),
        'dof': np.array(['front', 'rear', 'top']),
        'warning_upper': warning_upper,
        'warning_lower': np.repeat((psd / 2)[:, np.newaxis], len(f), axis=1),
        'abort_upper': np.repeat((4 * psd)[:, np.newaxis], len(f), axis=1),
        'abort_lower': np.repeat((psd / 4)[:, np.newaxis], len(f), axis=1),
    }


def write(folder: pathlib.Path = OUT) -> str:
    """Convert the specification into `folder`; returns the content hash."""
    from scipy.io import savemat

    from hertzforge.io import reference_convert

    with tempfile.TemporaryDirectory() as scratch:
        mat = os.path.join(scratch, 'three_channel.mat')
        savemat(mat, specification())
        return reference_convert.from_mat(mat, folder, df_hz=DF_HZ, lines=LINES,
                                          created_unix_secs=CREATED)


if __name__ == '__main__':
    print(write())
    sys.exit(0)
