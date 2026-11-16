"""Write the golden drive folders, `schema/golden/drive/basic/` and
`schema/golden/drive/fixed/`:

    python schema/make_golden_drive.py

`basic`: two channels, 401 samples at 1000 samples per second, written by
the package's own writer (`hertzforge.io.drive_convert.write`) with its time
and build fields fixed, so the folder is the same bytes each time it is
made. Each channel is `A cos(2 pi m (t - T/2) / T) sin(pi t / T)^2`,
T = 0.4 s: it starts and ends at zero, and integrated as an acceleration it
ends with zero velocity and zero displacement, so it passes the control
service's check. Its `source` names a delimited file of the same values at
a fixed path, with that file's SHA-256. The values are rounded to float32
(and stored as float64), so the bytes do not depend on the platform's sine.

`fixed`: what `hertzforge.io.drive_convert.fix_ends` writes from a drive
whose ends are not at rest (`ends_values`: two channels, 301 samples at
1000 samples per second, a 370 Hz tone over an offset and a 370 Hz cosine,
each starting and ending away from zero). The folder it was fixed from and
the build are fixed in the sidecar, so the folder is the same bytes each
time it is made. The control service's tests check that it passes the
check.

`tests/test_drive_convert.py` checks the committed folders against this
script and against the values below; `docs/drive-file.md` describes them.
"""

from __future__ import annotations

import hashlib
import pathlib
import shutil

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent / 'golden' / 'drive' / 'basic'
FIXED = HERE.parent / 'fixed'
RATE = 1000.0
SAMPLES = 401
#: each channel's name, amplitude in volts and cycles over the record
CHANNELS = (('Drive 1', 0.5, 8), ('Drive 2', 0.25, 20))
SOURCE_PATH = '/Users/someone/tests/bracket/drive.csv'
#: the fixed folder: its rate, samples and channels (name, offset, sine
#: amplitude, cosine amplitude; volts), and the folder it was fixed from
ENDS_RATE = 1000.0
ENDS_SAMPLES = 301
ENDS_HZ = 370.0
ENDS_CHANNELS = (('Offset', 0.1, 0.4, 0.0), ('Cosine', 0.0, 0.0, 0.5))
ENDS_FOLDER = '/Users/someone/Hertzforge_Data/drives/ends'
CREATED_UTC = '2026-11-15T00:00:00Z'
TOOL = 'hertzforge 0.0.0'


def _platform_free(values: np.ndarray) -> np.ndarray:
    """`values` rounded to float32 (and held as float64), so a sine that
    differs in its last place on another platform writes the same bytes."""
    return values.astype(np.float32).astype(np.float64)


def known_values() -> np.ndarray:
    """The golden folder's values, `(samples, channels)` float64 volts."""
    t = np.arange(SAMPLES) / RATE
    period = (SAMPLES - 1) / RATE
    window = np.sin(np.pi * t / period) ** 2
    window[0] = window[-1] = 0.0
    return _platform_free(np.stack(
        [amplitude * np.cos(2 * np.pi * cycles * (t - period / 2) / period) * window
         for _name, amplitude, cycles in CHANNELS], axis=1))


def source_text() -> str:
    """The delimited file the golden folder's `source` names."""
    values = known_values()
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
        created_utc=CREATED_UTC, tool=TOOL)


def ends_values() -> np.ndarray:
    """The drive the fixed folder was fixed from, `(samples, channels)`
    float64 volts: it starts and ends away from zero."""
    t = np.arange(ENDS_SAMPLES) / ENDS_RATE
    return _platform_free(np.stack([offset + sine * np.sin(2 * np.pi * ENDS_HZ * t)
                                    + cosine * np.cos(2 * np.pi * ENDS_HZ * t)
                                    for _name, offset, sine, cosine in ENDS_CHANNELS],
                                   axis=1))


def fixed(out: pathlib.Path | None = None) -> None:
    """Write the fixed folder at `out` (the committed folder when omitted):
    `fix_ends` of `ends_values`, with the folder it was fixed from and the
    build fixed in its sidecar."""
    import json
    import tempfile

    from hertzforge.io import drive_convert

    out = FIXED if out is None else pathlib.Path(out)
    if out.exists():
        shutil.rmtree(out)
    out.parent.mkdir(parents=True, exist_ok=True)
    names = [name for name, *_rest in ENDS_CHANNELS]
    with tempfile.TemporaryDirectory() as scratch:
        source = drive_convert.write(
            ends_values(), ENDS_RATE, names, out=pathlib.Path(scratch) / 'ends',
            source={'path': None, 'sha256': None, 'format': 'npy', 'selections': []},
            created_utc=CREATED_UTC, tool=TOOL)
        drive_convert.fix_ends(source, out=out, created_utc=CREATED_UTC)
    side = out / drive_convert.SIDECAR_FILE
    sidecar = json.loads(side.read_text(encoding='utf-8'))
    sidecar['fixes'][-1]['from']['folder'] = ENDS_FOLDER
    sidecar['tool'] = TOOL
    side.write_text(json.dumps(sidecar, indent=2, ensure_ascii=False) + '\n', encoding='utf-8')


if __name__ == '__main__':
    main()
    fixed()
