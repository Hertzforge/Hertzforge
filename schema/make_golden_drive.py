"""Write the golden drive folder, `schema/golden/drive/basic/`:

    python schema/make_golden_drive.py

Two channels, 401 samples at 1000 samples per second, written by the
package's own writer (`hertzforge.io.drive_convert.write`) with its time and
build fields fixed, so the folder is the same bytes each time it is made.
Each channel is `A cos(2 pi m (t - T/2) / T) sin(pi t / T)^2`, T = 0.4 s:
it starts and ends at zero, and integrated as an acceleration it ends with
zero velocity and zero displacement, so it passes the control service's
check. Its `source` names a delimited file of the same values at a fixed
path, with that file's SHA-256. `tests/test_drive_convert.py` checks the
committed folder against this script and against the values below;
`docs/drive-file.md` describes it.
"""

from __future__ import annotations

import hashlib
import pathlib
import shutil

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent / 'golden' / 'drive' / 'basic'
RATE = 1000.0
SAMPLES = 401
#: each channel's name, amplitude in volts and cycles over the record
CHANNELS = (('Drive 1', 0.5, 8), ('Drive 2', 0.25, 20))
SOURCE_PATH = '/Users/someone/tests/bracket/drive.csv'


def known_values() -> np.ndarray:
    """The golden folder's values, `(samples, channels)` float64 volts."""
    t = np.arange(SAMPLES) / RATE
    period = (SAMPLES - 1) / RATE
    window = np.sin(np.pi * t / period) ** 2
    window[0] = window[-1] = 0.0
    return np.stack([amplitude * np.cos(2 * np.pi * cycles * (t - period / 2) / period)
                     * window for _name, amplitude, cycles in CHANNELS], axis=1)


def source_text() -> str:
    """The delimited file the golden folder's `source` names."""
    values = known_values().astype(np.float32).astype(np.float64)
    lines = ['Time (s),' + ','.join(f'{name} (V)' for name, _a, _c in CHANNELS)]
    lines += [','.join([repr(k / RATE)] + [repr(float(v)) for v in row])
              for k, row in enumerate(values)]
    return '\n'.join(lines) + '\n'


def main(out: pathlib.Path | None = None) -> None:
    """Write the folder at `out` (the committed folder when omitted)."""
    from hertzforge.io import drive_convert

    out = HERE if out is None else pathlib.Path(out)
    if out.exists():
        shutil.rmtree(out)
    out.parent.mkdir(parents=True, exist_ok=True)
    names = [name for name, _a, _c in CHANNELS]
    columns = [f'column:{k + 2}' for k in range(len(names))]
    drive_convert.write(
        known_values(), RATE, names, out=out, channel_sources=columns,
        source={'path': SOURCE_PATH,
                'sha256': hashlib.sha256(source_text().encode('utf-8')).hexdigest(),
                'format': 'delimited',
                'selections': [{'signal': c, 'name': n} for c, n in zip(columns, names)]},
        created_utc='2026-11-15T00:00:00Z', tool='hertzforge 0.0.0')


if __name__ == '__main__':
    main()
