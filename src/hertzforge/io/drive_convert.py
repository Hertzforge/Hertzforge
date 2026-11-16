"""Drive files for Playback: any supported file in, one native drive file out.

Playback plays a drive the operator made, open loop, on as many drives as
the station has, and records every input and every drive. The control
service reads one form of drive only, the native drive folder laid out in
`docs/drive-file.md`; every other form is read here, on the analysis side,
by the readers the package already has, and written to that folder. The
control service checks the folder again before it arms.

- `inspect` says what a file holds: each signal that could be a drive,
  with its name, length, the rate the file states (if any), the unit the
  file states (if any) and where it is in the file.
- `convert` takes the signals chosen, in drive order, and writes a new
  drive folder; `fix_ends` writes a corrected copy of a drive folder
  whose ends are not at rest; `read` reads a drive folder back, checked.

Forms read (`SUFFIXES`): delimited text (CSV, TSV, TXT; a time column is
optional), MATLAB `.mat` version 5 and 7.3, NumPy `.npy` and `.npz`
(never unpickled), HDF5 `.h5` and `.hdf5` (every 1-D or 2-D numeric
dataset), `.hforge` time histories, universal file dataset 58 time
records, Excel `.xlsx` and WAV; and the time histories of an open project,
passed as a mapping of name to object.

**Signals.** A 1-D array is one signal. A 2-D array offers its columns,
samples down the rows, except a single row, which is one signal. A
MATLAB 7.3 file's arrays are read in MATLAB's own orientation. A scalar
named for a rate (`fs`, `sample_rate`, `rate_hz`, ...) or a step (`dt`,
...), or a 1-D array named for time, states the file's rate.

**Refusals**, each by name: a selection the file does not hold or that
repeats; signals of unequal length; signals whose rates differ, or a rate
the file states that disagrees with the rate given (by more than
`RATE_TOLERANCE` of it; the given rate is then written); no rate at all;
a time column whose step is not even; a unit that is not a voltage (V and
mV, or any voltage unit, are converted to volts; a signal with no unit is
taken as volts and the notes say so); a value that is not a finite number,
or one too large for the drive output's 32-bit floating point; a NumPy
array of Python objects, which is never unpickled; an output folder that
already exists.

**Rates.** A rate taken from a file is kept to ten significant digits,
so a time column's float rounding does not reach the drive file; a rate
given is written as given. Nothing is resampled.

**Fix ends.** The control service refuses a drive whose ends are not at
rest: treated as an acceleration and integrated with a plain trapezoid,
the record must start and end with zero acceleration, velocity and
displacement. `fix_ends` writes a new folder (the original is untouched)
with a half-cosine taper at each end, then subtracts
`c(t) = a sin(pi t / T) + b sin(2 pi t / T)`, zero at both ends, with `a`
and `b` solved so the corrected record ends with zero velocity and zero
displacement. The taper, `a`, `b` and the change in rms of each channel
are recorded in the new sidecar's `fixes`.
"""

from __future__ import annotations

import contextlib
import datetime as _dt
import hashlib
import itertools
import json
import os
import re
import shutil
import warnings
from collections.abc import Callable, Iterator, Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any

import numpy as np

#: the file names inside a drive folder
BINARY_FILE = 'drive.bin'
SIDECAR_FILE = 'drive.json'
#: the format the sidecar names, and its version
FORMAT = 'hertzforge-drive'
FORMAT_VERSION = 1
#: the eight bytes `drive.bin` starts with
MAGIC = b'HFDRV01\x00'
#: the bytes of one sample: a little-endian float64, so the values the
#: control service checks are the values written here
SAMPLE_BYTES = 8
#: a sample's type in `drive.bin`
SAMPLE_DTYPE = '<f8'
#: the unit every drive channel is written in
UNIT = 'V'
#: the folder under the data folder that drive folders are made in
DRIVES_FOLDER = 'drives'
#: samples per channel converted and written at a time
CHUNK_SAMPLES = 1 << 16
#: a stated rate and a given rate agree within this fraction of the given
RATE_TOLERANCE = 1e-4
#: a time column's steps are even within this fraction of their mean
TIME_STEP_TOLERANCE = 1e-6
#: the default taper of `fix_ends`: this fraction of the record, within
#: the two bounds in seconds
TAPER_FRACTION = 0.01
TAPER_MIN_S = 0.01
TAPER_MAX_S = 1.0

#: each form by suffix, as `inspect` and the sidecar's `source.format` name it
SUFFIXES = {'.csv': 'delimited', '.tsv': 'delimited', '.txt': 'delimited',
            '.tab': 'delimited', '.asc': 'delimited', '.prn': 'delimited',
            '.mat': 'mat', '.npy': 'npy', '.npz': 'npz', '.h5': 'hdf5',
            '.hdf5': 'hdf5', '.hforge': 'hforge', '.unv': 'unv', '.uff': 'unv',
            '.xlsx': 'xlsx', '.wav': 'wav'}

#: names that state a rate in samples per second, or a step in seconds,
#: compared lower-cased with spaces and dashes as underscores
RATE_NAMES = frozenset({'fs', 'sample_rate', 'samplerate', 'sampling_rate',
                        'sampling_frequency', 'rate', 'rate_hz', 'sr', 'fs_hz'})
STEP_NAMES = frozenset({'dt', 'time_step', 'timestep', 'sample_interval', 'delta_t'})
#: names that make a 1-D array a time vector
TIME_NAMES = frozenset({'t', 'time', 'times', 'time_s', 'seconds'})


class DriveFileError(ValueError):
    """A drive that is not written or not read, and why, in words a person
    can act on."""


@dataclass
class Signal:
    """One signal a file offers as a drive.

    `key` is what a selection names; `where` says where it is in words;
    `rate_hz` and `unit` are what the file states, or None; `problem` is
    why it cannot be a drive (its unit), or None.
    """

    key: str
    name: str
    samples: int
    rate_hz: float | None
    unit: str | None
    where: str
    problem: str | None = None

    def as_dict(self) -> dict[str, Any]:
        """The signal as the analysis service sends it."""
        return {'key': self.key, 'name': self.name, 'samples': self.samples,
                'rate_hz': self.rate_hz, 'unit': self.unit, 'where': self.where,
                'problem': self.problem}


@dataclass
class Inspection:
    """What `inspect` found: the source, its signals and notes."""

    path: str | None
    format: str
    signals: list[Signal]
    notes: list[str] = field(default_factory=list)

    def as_dict(self) -> dict[str, Any]:
        """The inspection as the analysis service sends it."""
        rates = {s.rate_hz for s in self.signals if s.rate_hz is not None}
        return {'source': {'path': self.path, 'format': self.format},
                'signals': [s.as_dict() for s in self.signals],
                'rate_hz': rates.pop() if len(rates) == 1 else None,
                'notes': list(self.notes)}


@dataclass
class DriveFile:
    """A drive folder as read: `values` is `(samples, channels)` float64,
    volts, and `sidecar` is `drive.json` whole."""

    folder: str
    values: np.ndarray
    rate_hz: float
    names: list[str]
    sidecar: dict[str, Any]


@dataclass
class Conversion:
    """What `convert` wrote: the folder, its sidecar and the notes; with
    `fix_ends`, the corrected folder's summary as `fixed`."""

    folder: str
    sidecar: dict[str, Any]
    notes: list[str] = field(default_factory=list)
    fixed: dict[str, Any] | None = None

    def as_dict(self) -> dict[str, Any]:
        """The conversion as the analysis service sends it."""
        out: dict[str, Any] = {'folder': self.folder, 'drive': self.sidecar,
                               'notes': list(self.notes)}
        if self.fixed is not None:
            out['fixed'] = self.fixed
        return out


# ---- the source, open --------------------------------------------------------

@dataclass
class _Held:
    """A signal with the means to read it: `fetch(start, stop)` gives its
    samples in the file's numbers, `scale` takes them to volts."""

    signal: Signal
    fetch: Callable[[int, int], np.ndarray]
    scale: float = 1.0


@dataclass
class _Source:
    path: str | None
    format: str
    held: list[_Held] = field(default_factory=list)
    notes: list[str] = field(default_factory=list)
    project_file: str | None = None

    def add(self, signal: Signal, fetch: Callable[[int, int], np.ndarray],
            scale: float = 1.0) -> None:
        self.held.append(_Held(signal, fetch, scale))


def _tool() -> str:
    from .. import __version__
    return f'hertzforge {__version__}'


def _utc_now() -> str:
    return _dt.datetime.now(_dt.UTC).strftime('%Y-%m-%dT%H:%M:%SZ')


def _snap(rate: float) -> float:
    """A rate read from a file, kept to ten significant digits."""
    return float(f'{float(rate):.10g}')


def _plain(name: str) -> str:
    return re.sub(r'[\s\-]+', '_', name.strip().lower())


def _decide(stated: str | None) -> tuple[float | None, str | None]:
    """A unit the file states as (scale to volts, None), or (None, why it
    is not a drive's unit). No unit is volts."""
    from ..units import dimension_of, si_factor

    if stated is None or not str(stated).strip():
        return 1.0, None
    stated = str(stated).strip()
    dim = dimension_of(stated)
    if dim == 'voltage':
        return float(si_factor(stated, 'voltage')), None
    what = f', which is a {dim} unit' if dim else ', which is not a unit this reads'
    return None, f'is in {stated!r}{what}; a drive is in volts (V or mV)'


def _matrix(source: _Source, data: Any, key: str, where: str, name: str, *,
            rate: float | None, units: Sequence[str | None] | str | None = None,
            transposed: bool = False, scale: float = 1.0) -> None:
    """Add a 1-D or 2-D numeric array's signals: a 1-D array, or a single
    row, is one; otherwise each column is one, samples down the rows.
    `data` may be an array, a memory map or an HDF5 dataset; with
    `transposed` its axes are reversed (a MATLAB 7.3 array)."""
    shape = tuple(int(n) for n in data.shape)
    if transposed:
        shape = shape[::-1]
    if len(shape) == 1 or (len(shape) == 2 and shape[0] == 1):
        n = shape[-1]

        def one(a: int, b: int) -> np.ndarray:
            if len(data.shape) == 1:
                return np.asarray(data[a:b], dtype=np.float64)
            return np.asarray(data[a:b, 0] if transposed else data[0, a:b], dtype=np.float64)

        unit = units if isinstance(units, str) or units is None else (units[0] if units
                                                                         else None)
        _add(source, Signal(key, name, n, rate, unit, where), one, scale)
        return
    n, columns = shape
    if isinstance(units, str) or units is None:
        units = [units] * columns
    for c in range(columns):
        def column(a: int, b: int, c: int = c) -> np.ndarray:
            part = data[c, a:b] if transposed else data[a:b, c]
            return np.asarray(part, dtype=np.float64)

        label = name if columns == 1 else f'{name} {c + 1}'
        unit = units[c] if c < len(units) else None
        _add(source, Signal(f'{key}:column:{c + 1}', label, n, rate, unit,
                            f'{where}, column {c + 1}'), column, scale)


def _add(source: _Source, signal: Signal, fetch: Callable[[int, int], np.ndarray],
         scale: float = 1.0) -> None:
    factor, problem = _decide(signal.unit)
    signal.problem = problem
    source.add(signal, fetch, scale * (factor if factor is not None else 1.0))


def _numeric(data: Any) -> bool:
    dtype = getattr(data, 'dtype', None)
    return dtype is not None and dtype.kind in 'iuf' and len(data.shape) in (1, 2)


def _even_rate(time: np.ndarray, name: str) -> float:
    """The rate of a time vector, refused unless its steps are even."""
    time = np.asarray(time, dtype=np.float64).ravel()
    if time.size < 2:
        raise DriveFileError(f'{name}: a time column needs at least two samples')
    steps = np.diff(time)
    mean = float(steps.mean())
    if not np.all(np.isfinite(steps)) or mean <= 0 or float(
            np.max(np.abs(steps - mean))) > TIME_STEP_TOLERANCE * mean:
        k = int(np.argmax(np.abs(steps - mean))) if np.all(np.isfinite(steps)) else 0
        raise DriveFileError(
            f'{name}: the time column does not step evenly (step {k + 1} is '
            f'{steps[k]:.9g} s against a mean of {mean:.9g} s); a drive is played at one '
            'rate and is never resampled')
    return _snap(1.0 / mean)


def _variables(source: _Source, entries: Sequence[tuple[str, Any, bool, Any]],
               kind: str, attrs: Mapping[str, Any] | None = None) -> None:
    """Signals from named arrays (a .mat, .npz or HDF5 file): `entries` are
    (name, data, transposed, units). Scalars named for a rate or a step,
    and 1-D arrays named for time, state the rate; every other 1-D or 2-D
    numeric array offers its signals."""
    stated: list[tuple[float, str]] = []
    for label, value in (attrs or {}).items():
        _rate_scalar(stated, label, value, f'the attribute {label!r}')
    signals = []
    for name, data, transposed, units in entries:
        base = name.rsplit('/', 1)[-1]
        dtype_kind = data.dtype.kind
        if dtype_kind == 'c':
            source.notes.append(f'{name} is complex and was left out: a drive is real')
            continue
        if dtype_kind not in 'iuf':
            continue
        size = int(np.prod(data.shape)) if data.shape else 1
        if len(data.shape) > 2 and size > 1:
            source.notes.append(f'{name} has {len(data.shape)} dimensions and was left out: '
                                'a drive is a 1-D array or the columns of a 2-D one')
            continue
        if size == 1:
            _rate_scalar(stated, base, np.asarray(data[()]).ravel()[0]
                         if data.shape else data[()], name)
            continue
        if _plain(base) in TIME_NAMES and (len(data.shape) == 1 or min(data.shape) == 1):
            stated.append((_even_rate(np.asarray(data[()]), name), f'the time vector {name!r}'))
            continue
        signals.append((name, data, transposed, units))
    rates = {r for r, _w in stated}
    if len(rates) > 1:
        said = ' and '.join(f'{r:g} from {w}' for r, w in stated)
        raise DriveFileError(f'the file states two rates, {said}; one of them is wrong, and '
                             'this cannot tell which')
    rate = rates.pop() if rates else None
    for name, data, transposed, units in signals:
        _matrix(source, data, f'{kind}:{name}', f'{kind} {name!r}', name.rsplit('/', 1)[-1],
                rate=rate, units=units, transposed=transposed)


def _rate_scalar(stated: list[tuple[float, str]], name: str, value: Any, where: str) -> None:
    key = _plain(str(name))
    try:
        number = float(np.asarray(value).ravel()[0])
    except (TypeError, ValueError, IndexError):
        return
    if key in RATE_NAMES and np.isfinite(number) and number > 0:
        stated.append((_snap(number), where))
    elif key in STEP_NAMES and np.isfinite(number) and number > 0:
        stated.append((_snap(1.0 / number), where))


def _h5_units(dataset: Any) -> Any:
    for name in ('unit', 'units'):
        if name in dataset.attrs:
            value = dataset.attrs[name]
            if isinstance(value, bytes):
                return value.decode('utf-8', 'replace')
            if isinstance(value, str):
                return value
            values = np.asarray(value).ravel()
            return [v.decode('utf-8', 'replace') if isinstance(v, bytes) else str(v)
                    for v in values]
    return None


def _open_h5(source: _Source, path: str, stack: contextlib.ExitStack, *,
             matlab: bool) -> None:
    import h5py

    f = stack.enter_context(h5py.File(path, 'r'))
    entries = []

    def visit(name: str, item: Any) -> None:
        if name.startswith('#') or not isinstance(item, h5py.Dataset):
            return
        if item.dtype.kind not in 'iufc' or len(item.shape) > 2:
            return
        if matlab and 'MATLAB_class' in item.attrs and item.attrs['MATLAB_class'] in (
                b'char', b'logical', b'cell'):
            return
        entries.append(('/' + name if not matlab else name, item, matlab,
                        None if matlab else _h5_units(item)))

    f.visititems(visit)
    attrs = {k: v for k, v in f.attrs.items()} if not matlab else None
    _variables(source, entries, 'variable' if matlab else 'dataset', attrs)


def _open_mat(source: _Source, path: str, stack: contextlib.ExitStack) -> None:
    import h5py

    if h5py.is_hdf5(path):
        source.format = 'mat73'
        _open_h5(source, path, stack, matlab=True)
        return
    from scipy.io import loadmat

    from . import matlab

    source.format = 'mat5'
    raw = {k: v for k, v in loadmat(path).items() if not k.startswith('__')}
    if matlab.STAMP in raw:
        held = matlab.load(path)
        _objects(source, held if isinstance(held, Mapping) else
                 {os.path.splitext(os.path.basename(path))[0]: held})
        return
    entries = [(name, np.asarray(value), False, None) for name, value in raw.items()
               if isinstance(value, np.ndarray) and value.dtype.kind in 'iufc']
    _variables(source, entries, 'variable')


def _open_npy(source: _Source, path: str) -> None:
    from . import plain_numpy

    try:
        dtype = plain_numpy.npy_dtype(path)
    except ValueError:
        raise DriveFileError(f'{path} is not a NumPy file') from None
    if dtype.hasobject:
        raise DriveFileError(f'{path} holds Python objects, not plain numbers. Reading them '
                             'could run code from the file, so it was not read.')
    data = np.load(path, mmap_mode='r', allow_pickle=False)
    if not _numeric(data):
        raise DriveFileError(f'{path} holds a {data.ndim}-D {data.dtype} array; a drive is a '
                             '1-D or 2-D array of real numbers')
    stem = os.path.splitext(os.path.basename(path))[0]
    _matrix(source, data, 'array', 'the array', stem, rate=None)


def _open_npz(source: _Source, path: str, stack: contextlib.ExitStack) -> None:
    from . import plain_numpy

    npz = stack.enter_context(plain_numpy.PlainNpz(path))
    try:
        entries = [(name, np.asarray(npz[name]), False, None) for name in npz.files]
    except ValueError as error:
        raise DriveFileError(str(error)) from None
    _variables(source, entries, 'array')


def _objects(source: _Source, objects: Mapping[str, Any]) -> None:
    """The records of the time histories among `objects`, by name."""
    from ..core.data import TimeHistory

    for name, obj in objects.items():
        if not isinstance(obj, TimeHistory):
            source.notes.append(f'{name!r} is a {type(obj).__name__}, not a time history, '
                                'and was left out')
            continue
        _history(source, obj, f'object:{name}', f'the object {name!r}', str(name))


def _history(source: _Source, history: Any, key: str, where: str, name: str,
             stated: Sequence[str | None] | None = None,
             labels: Sequence[str] | None = None,
             columns: Sequence[int] | None = None) -> None:
    """A signal per record of a `TimeHistory`, its values as the history
    holds them (SI where its unit is declared, the file's numbers where
    not). `stated` overrides the unit each record's file wrote."""
    from ..units import UNKNOWN

    try:
        rate = _snap(history.sample_rate)
    except ValueError as error:
        raise DriveFileError(f'{name}: {error}; a drive is played at one rate and is never '
                             'resampled') from None
    ordinate = history.ordinate
    count = ordinate.shape[0]
    for k in range(count):
        dim = history.ordinate_dim[k]
        unit = history.ordinate_unit[k]
        hint = history.dimension_hint[k] if history.dimension_hint else None
        said = stated[k] if stated is not None else unit
        scale, problem = 1.0, None
        if dim != UNKNOWN:
            # declared: the values are SI, a voltage in volts
            if dim != 'voltage':
                problem = (f'is in {said or unit!r}, which is a {dim} unit; a drive is in '
                           'volts (V or mV)')
        elif said:
            factor, problem = _decide(said)
            scale = factor if factor is not None else 1.0
        elif hint not in (None, '', 'voltage'):
            problem = f'is a {hint}, the file says; a drive is in volts (V or mV)'
        label = (labels[k] if labels is not None else
                 name if count == 1 else f'{name} {history.record_label(k)}')
        if columns is not None:
            sig_key, sig_where = f'column:{columns[k] + 1}', f'column {columns[k] + 1}'
        else:
            sig_key, sig_where = f'{key}:record:{k + 1}', f'{where}, record {k + 1}'
        signal = Signal(sig_key, label, int(ordinate.shape[1]), rate, said, sig_where, problem)

        def fetch(a: int, b: int, k: int = k) -> np.ndarray:
            return np.asarray(ordinate[k, a:b], dtype=np.float64)

        source.add(signal, fetch, scale)


def _open_delimited(source: _Source, path: str) -> None:
    from . import delimited

    layout = delimited.text_layout(path)
    has_rate = layout.time_column is not None or layout.sample_rate is not None
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter('always')
        if has_rate:
            history = delimited.read_delimited(path)
        else:
            history = delimited.read_delimited(path, time_column=False, sample_rate=1.0)
    source.notes.extend(str(w.message) for w in caught)
    columns = [k for k in range(len(layout.columns)) if k != layout.time_column]
    names = [layout.columns[k] for k in columns]
    _history(source, history, '', '', os.path.basename(path),
             stated=[layout.units[k] for k in columns], labels=names, columns=columns)
    if not has_rate:
        for held in source.held:
            held.signal.rate_hz = None


def _open_unv(source: _Source, path: str) -> None:
    from . import unv

    held = unv.load(path)
    stem = os.path.splitext(os.path.basename(path))[0]
    _objects(source, held if isinstance(held, Mapping) else {stem: held})


def _open_hforge(source: _Source, path: str) -> None:
    from . import native

    held = native.load(path)
    stem = os.path.splitext(os.path.basename(path))[0]
    _objects(source, held if isinstance(held, Mapping) else {stem: held})


def _cell_number(value: Any) -> float | None:
    if value is None or (isinstance(value, str) and not value.strip()):
        return float('nan')
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return float(value)
    try:
        return float(str(value).strip())
    except ValueError:
        return None


def _open_xlsx(source: _Source, path: str) -> None:
    from openpyxl import load_workbook
    from openpyxl.utils import get_column_letter

    from .delimited import TIME_NAMES as TEXT_TIME_NAMES
    from .delimited import _name_and_unit

    book = load_workbook(path, read_only=True, data_only=True)
    try:
        for sheet in book.worksheets:
            rows = [list(r) for r in sheet.iter_rows(values_only=True)]
            rows = [r for r in rows if any(v is not None and str(v).strip() for v in r)]
            if len(rows) < 2:
                continue
            header = None
            if any(isinstance(v, str) and _cell_number(v) is None for v in rows[0]):
                header, rows = rows[0], rows[1:]
            width = max(len(r) for r in rows)
            values = np.full((len(rows), width), np.nan)
            for i, row in enumerate(rows):
                for c, v in enumerate(row):
                    number = _cell_number(v)
                    if number is None:
                        raise DriveFileError(
                            f'{os.path.basename(path)}, sheet {sheet.title!r}, cell '
                            f'{get_column_letter(c + 1)}{i + 1 + (header is not None)}: '
                            f'{v!r} is not a number')
                    values[i, c] = number
            names, units = [], []
            for c in range(width):
                text = str(header[c]).strip() if header and c < len(header) and \
                    header[c] is not None else ''
                name, unit = _name_and_unit(text) if text else (
                    f'column {get_column_letter(c + 1)}', None)
                names.append(name)
                units.append(unit)
            times = [c for c in range(width) if names[c].lower() in TEXT_TIME_NAMES]
            rate = None
            if times:
                rate = _even_rate(values[:, times[0]], f'sheet {sheet.title!r}, '
                                  f'column {get_column_letter(times[0] + 1)}')
            for c in range(width):
                if c in times[:1]:
                    continue
                letter = get_column_letter(c + 1)
                data = values[:, c].copy()
                _add(source, Signal(f'sheet:{sheet.title}:column:{letter}', names[c],
                                    len(data), rate, units[c],
                                    f'sheet {sheet.title!r}, column {letter}'),
                     lambda a, b, data=data: data[a:b])
    finally:
        book.close()


def _open_wav(source: _Source, path: str) -> None:
    from scipy.io import wavfile

    rate, data = wavfile.read(path, mmap=True)
    scale = 1.0
    if data.dtype.kind in 'iu':
        bits = data.dtype.itemsize * 8
        scale = 1.0 / float(2 ** (bits - 1))
        source.notes.append(f'{os.path.basename(path)} holds {bits}-bit integer samples, '
                            f'scaled to [-1, 1): full scale is 1 V')
    if data.dtype.kind == 'u':
        offset = float(2 ** (data.dtype.itemsize * 8 - 1))
        raw = data

        class _Centred:
            shape = raw.shape

            def __getitem__(self, index: Any) -> np.ndarray:
                return np.asarray(raw[index], dtype=np.float64) - offset

        data = _Centred()
    stem = os.path.splitext(os.path.basename(path))[0]
    before = len(source.held)
    _matrix(source, data, 'channel', 'channel', stem, rate=_snap(rate), scale=scale)
    # a WAV's channels are its columns, named and placed as channels
    for k, held in enumerate(source.held[before:]):
        held.signal.key = f'channel:{k + 1}'
        held.signal.where = f'channel {k + 1}'
        held.signal.name = stem if len(source.held) - before == 1 else f'{stem} {k + 1}'


@contextlib.contextmanager
def _opened(source: str | os.PathLike | Mapping[str, Any],
            project_file: str | None = None) -> Iterator[_Source]:
    """A source open for reading, its files closed after."""
    with contextlib.ExitStack() as stack:
        if isinstance(source, Mapping):
            opened = _Source(project_file, 'project', project_file=project_file)
            _objects(opened, source)
            yield opened
            return
        path = os.path.abspath(os.path.expanduser(os.fspath(source)))
        if not os.path.isfile(path):
            raise FileNotFoundError(f'no file at {path}')
        suffix = os.path.splitext(path)[1].lower()
        form = SUFFIXES.get(suffix)
        if form is None:
            raise DriveFileError(
                f'{os.path.basename(path)}: a drive is read from '
                f'{", ".join(sorted(SUFFIXES))}; {suffix or "no suffix"} is not one')
        opened = _Source(path, form)
        if form == 'delimited':
            _open_delimited(opened, path)
        elif form == 'mat':
            _open_mat(opened, path, stack)
        elif form == 'npy':
            _open_npy(opened, path)
        elif form == 'npz':
            _open_npz(opened, path, stack)
        elif form == 'hdf5':
            _open_h5(opened, path, stack, matlab=False)
        elif form == 'hforge':
            _open_hforge(opened, path)
        elif form == 'unv':
            _open_unv(opened, path)
        elif form == 'xlsx':
            _open_xlsx(opened, path)
        else:
            _open_wav(opened, path)
        _unique_names(opened)
        yield opened


def _unique_names(source: _Source) -> None:
    seen: dict[str, int] = {}
    for held in source.held:
        seen[held.signal.name] = seen.get(held.signal.name, 0) + 1
    for held in source.held:
        if seen[held.signal.name] > 1:
            held.signal.name = f'{held.signal.name} ({held.signal.where})'


# ---- inspect -----------------------------------------------------------------

def inspect(source: str | os.PathLike | Mapping[str, Any],
            project_file: str | None = None) -> Inspection:
    """What a file, or a set of a project's objects, holds as drives.

    Parameters
    ----------
    source : str, os.PathLike or mapping
        The file's path on this computer, or a mapping of name to object
        (the time histories of an open project).
    project_file : str, optional
        The project's file, for objects, as their provenance.

    Returns
    -------
    Inspection
        Each signal (`Signal`) and the notes the readers made.

    Raises
    ------
    FileNotFoundError
        When there is no such file.
    ValueError
        When the file cannot be read as a drive, saying why.
    """
    with _opened(source, project_file) as opened:
        return Inspection(opened.path, opened.format,
                          [held.signal for held in opened.held], list(opened.notes))


# ---- the folder ----------------------------------------------------------------

def _claim(out: str | os.PathLike | None, parent: str, stem: str) -> str:
    """Make the new folder: `out` exactly, refused when it exists; or a
    free name in `parent` from `stem` (`stem`, `stem-2`, ...). Made with a
    call that fails if it exists, so no folder is ever written over."""
    if out is not None:
        path = os.path.abspath(os.path.expanduser(os.fspath(out)))
        os.makedirs(os.path.dirname(path), exist_ok=True)
        try:
            os.mkdir(path)
        except FileExistsError:
            raise DriveFileError(f'{path} already exists; a drive folder is never written '
                                 'over, so name a new one') from None
        return path
    os.makedirs(parent, exist_ok=True)
    stem = re.sub(r'[^A-Za-z0-9._-]+', '_', stem).strip('._-')[:60] or 'drive'
    k = 1
    while True:
        path = os.path.join(parent, stem if k == 1 else f'{stem}-{k}')
        try:
            os.mkdir(path)
        except FileExistsError:
            k += 1
            continue
        return path


def default_folder() -> str:
    """Where new drive folders are made: `drives` in the data folder
    (`hertzforge.io.data_folder.data_root`), which is local to the
    computer that runs the test.

    Returns
    -------
    str
        The folder's absolute path. It need not exist yet.
    """
    from .data_folder import data_root

    return os.path.join(data_root(), DRIVES_FOLDER)


def _write_folder(folder: str, chunks: Iterator[np.ndarray], samples: int,
                  channels: Sequence[Mapping[str, Any]], rate_hz: float,
                  source: Mapping[str, Any], fixes: Sequence[Mapping[str, Any]],
                  created_utc: str | None, tool: str | None) -> dict[str, Any]:
    """Write `drive.bin` from `chunks` (each `(n, channels)`, volts) then
    `drive.json`, each to a temporary name renamed into place."""
    digest = hashlib.sha256()
    written = 0
    binary = os.path.join(folder, BINARY_FILE)
    with open(binary + '.partial', 'wb') as f:
        f.write(MAGIC)
        digest.update(MAGIC)
        for chunk in chunks:
            data = np.ascontiguousarray(chunk, dtype=SAMPLE_DTYPE)
            if data.ndim != 2 or data.shape[1] != len(channels):
                raise DriveFileError('a block of the drive is not samples by channels')
            raw = data.tobytes()
            f.write(raw)
            digest.update(raw)
            written += data.shape[0]
    if written != samples:
        raise DriveFileError(f'{written} samples were written of {samples}')
    os.replace(binary + '.partial', binary)
    sidecar = {
        'format': FORMAT,
        'version': FORMAT_VERSION,
        'rate_hz': float(rate_hz),
        'samples': int(samples),
        'channels': [{'name': str(c['name']), 'unit': UNIT, 'source': c['source']}
                     for c in channels],
        'sha256': digest.hexdigest(),
        'source': dict(source),
        'fixes': [dict(fix) for fix in fixes],
        'tool': tool if tool is not None else _tool(),
        'created_utc': created_utc if created_utc is not None else _utc_now(),
    }
    side = os.path.join(folder, SIDECAR_FILE)
    with open(side + '.partial', 'w', encoding='utf-8') as f:
        f.write(json.dumps(sidecar, indent=2, ensure_ascii=False) + '\n')
    os.replace(side + '.partial', side)
    return sidecar


def write(values: np.ndarray, rate_hz: float, names: Sequence[str], *,
          source: Mapping[str, Any], out: str | os.PathLike | None = None,
          fixes: Sequence[Mapping[str, Any]] = (), channel_sources: Sequence[Any] | None = None,
          created_utc: str | None = None, tool: str | None = None) -> str:
    """Write a drive folder from values in hand.

    Parameters
    ----------
    values : numpy.ndarray
        `(samples, channels)`, volts, every value finite.
    rate_hz : float
        Samples per second.
    names : sequence of str
        Each channel's name.
    source : mapping
        The sidecar's `source`: `path`, `sha256`, `format`, `selections`.
    out : str or os.PathLike, optional
        The folder to make; refused when it exists. A free name under
        `default_folder` when omitted.
    fixes : sequence of mapping, optional
        The sidecar's `fixes`.
    channel_sources : sequence, optional
        Each channel's `source`; its name when omitted.
    created_utc, tool : str, optional
        The sidecar's fields, the time now and this build when omitted
        (fixed for a golden folder).

    Returns
    -------
    str
        The folder written.
    """
    values = np.asarray(values, dtype=np.float64)
    if values.ndim == 1:
        values = values[:, None]
    if values.ndim != 2 or values.shape[1] != len(names) or values.shape[0] < 2:
        raise DriveFileError('a drive is samples by channels, at least two samples, with a '
                             'name for each channel')
    _check_finite(values, names, 0, rate_hz)
    folder = _claim(out, default_folder(), str(names[0]) if names else 'drive')
    channels = [{'name': n, 'source': s} for n, s in
                zip(names, channel_sources if channel_sources is not None else names,
                    strict=True)]
    try:
        _write_folder(folder, iter([values]), values.shape[0], channels, rate_hz, source,
                      fixes, created_utc, tool)
    except BaseException:
        shutil.rmtree(folder, ignore_errors=True)
        raise
    return folder


def _check_finite(block: np.ndarray, names: Sequence[str], start: int, rate: float) -> None:
    bad = ~np.isfinite(block) | (np.abs(np.nan_to_num(block)) > np.finfo(np.float32).max)
    if bad.any():
        sample, channel = (int(v[0]) for v in np.nonzero(bad))
        value = block[sample, channel]
        what = ('is not a finite number' if not np.isfinite(value) else
                'is too large for the drive output (32-bit floating point)')
        at = start + sample
        raise DriveFileError(f'{names[channel]}: sample {at + 1} (at {at / rate:.9g} s) is '
                             f'{float(value)!r}, which {what}')


def _file_sha256(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, 'rb') as f:
        for block in iter(lambda: f.read(1 << 20), b''):
            digest.update(block)
    return digest.hexdigest()


def _selection(item: Any) -> tuple[str, str | None]:
    if isinstance(item, str):
        return item, None
    if isinstance(item, Mapping) and isinstance(item.get('signal'), str):
        name = item.get('name')
        if name is not None and (not isinstance(name, str) or not name.strip()):
            raise DriveFileError(f'the selection of {item["signal"]!r} gives a name that is '
                                 'not text')
        return item['signal'], name.strip() if name else None
    raise DriveFileError('each selection is a signal key, or {"signal": key, "name": text}')


# ---- convert -------------------------------------------------------------------

def convert(source: str | os.PathLike | Mapping[str, Any], selections: Sequence[Any], *,
            rate_hz: float | None = None, out: str | os.PathLike | None = None,
            fix_ends: bool = False, taper_s: float | None = None,
            project_file: str | None = None, created_utc: str | None = None) -> Conversion:
    """Write the chosen signals, in drive order, as a new drive folder.

    Parameters
    ----------
    source : str, os.PathLike or mapping
        The file's path on this computer, or a mapping of name to object
        (the time histories of an open project).
    selections : sequence
        The drives in order: each a signal's `key` from `inspect`, or
        ``{"signal": key, "name": text}`` to name the channel.
    rate_hz : float, optional
        The rate; it must agree with any rate the file states. Needed
        when the file states none.
    out : str or os.PathLike, optional
        The folder to make; refused when it exists. A free name under
        `default_folder` when omitted.
    fix_ends : bool, optional
        Also write a corrected copy (`fix_ends`), returned as `fixed`.
    taper_s : float, optional
        The taper of that copy, in seconds.
    project_file : str, optional
        The project's file, for objects, as their provenance.
    created_utc : str, optional
        The sidecar's time, now when omitted.

    Returns
    -------
    Conversion
        The folder written, its sidecar and the notes.

    Raises
    ------
    FileNotFoundError
        When there is no such file.
    ValueError
        For every refusal the module docstring lists, by name. Nothing is
        left on disk then.
    """
    if not selections:
        raise DriveFileError('choose at least one signal as a drive')
    with _opened(source, project_file) as opened:
        by_key = {held.signal.key: held for held in opened.held}
        chosen: list[tuple[_Held, str]] = []
        seen: set[str] = set()
        for item in selections:
            key, name = _selection(item)
            if key not in by_key:
                raise DriveFileError(f'{key!r} is not a signal of this source; it holds '
                                     f'{len(by_key)}, as inspect lists them')
            if key in seen:
                raise DriveFileError(f'{key!r} is chosen twice; each signal drives once')
            seen.add(key)
            chosen.append((by_key[key], name or by_key[key].signal.name))
        names = [name for _h, name in chosen]
        doubled = sorted({n for n in names if names.count(n) > 1})
        if doubled:
            raise DriveFileError(f'two drives are named {doubled[0]!r}; name each one')
        notes = list(opened.notes)
        for held, name in chosen:
            if held.signal.problem:
                raise DriveFileError(f'{name} {held.signal.problem}')
            if held.signal.unit is None:
                notes.append(f'{name}: the file states no unit; taken as volts')
        lengths = {held.signal.samples for held, _n in chosen}
        if len(lengths) > 1:
            said = ', '.join(f'{n} has {h.signal.samples}' for h, n in chosen)
            raise DriveFileError(f'the drives differ in length ({said} samples); every drive '
                                 'plays the same number of samples')
        samples = lengths.pop()
        if samples < 2:
            raise DriveFileError(f'{names[0]} has {samples} sample; a drive needs at least two')
        rate = _rate(chosen, rate_hz, notes)
        stem = (os.path.splitext(os.path.basename(opened.path))[0]
                if opened.path and opened.format != 'project' else names[0])
        selected = [{'signal': h.signal.key, 'name': n} for h, n in chosen]
        folder = _claim(out, default_folder(), stem)
        try:
            digest = hashlib.sha256() if opened.format == 'project' else None

            def chunks() -> Iterator[np.ndarray]:
                for start in range(0, samples, CHUNK_SAMPLES):
                    stop = min(samples, start + CHUNK_SAMPLES)
                    block = np.empty((stop - start, len(chosen)))
                    for c, (held, _n) in enumerate(chosen):
                        part = held.fetch(start, stop)
                        if digest is not None:
                            digest.update(np.ascontiguousarray(part, dtype='<f8').tobytes())
                        block[:, c] = part * held.scale
                    _check_finite(block, names, start, rate)
                    yield block

            # a project's objects hash as the values chosen, in drive order,
            # a block at a time; a file hashes whole
            sha = None if digest is not None else _file_sha256(opened.path)
            sidecar_source: dict[str, Any] = {'path': opened.path, 'sha256': sha,
                                              'format': opened.format, 'selections': selected}
            sidecar = _write_folder(folder, chunks(), samples,
                                    [{'name': n, 'source': h.signal.key} for h, n in chosen],
                                    rate, sidecar_source, (), created_utc, None)
            if digest is not None:
                sidecar['source']['sha256'] = digest.hexdigest()
                _rewrite_sidecar(folder, sidecar)
        except BaseException:
            shutil.rmtree(folder, ignore_errors=True)
            raise
    conversion = Conversion(folder, sidecar, notes)
    if fix_ends:
        conversion.fixed = _fix_ends(folder, taper_s=taper_s, created_utc=created_utc)
    return conversion


def _rewrite_sidecar(folder: str, sidecar: Mapping[str, Any]) -> None:
    side = os.path.join(folder, SIDECAR_FILE)
    with open(side + '.partial', 'w', encoding='utf-8') as f:
        f.write(json.dumps(sidecar, indent=2, ensure_ascii=False) + '\n')
    os.replace(side + '.partial', side)


def _rate(chosen: Sequence[tuple[_Held, str]], rate_hz: float | None,
          notes: list[str]) -> float:
    stated = [(h.signal.rate_hz, n) for h, n in chosen if h.signal.rate_hz is not None]
    for (a, na), (b, nb) in itertools.pairwise(stated):
        if abs(a - b) > RATE_TOLERANCE * max(a, b):
            raise DriveFileError(f'{na} is at {a:g} samples per second and {nb} at {b:g}; '
                                 'every drive plays at one rate, and nothing is resampled')
    if rate_hz is not None:
        try:
            given = float(rate_hz)
        except (TypeError, ValueError):
            raise DriveFileError(f'the rate {rate_hz!r} is not a number') from None
        if not np.isfinite(given) or given <= 0:
            raise DriveFileError(f'the rate {given:g} is not a positive number of samples '
                                 'per second')
        if stated and abs(stated[0][0] - given) > RATE_TOLERANCE * given:
            raise DriveFileError(f'the file states {stated[0][0]:g} samples per second for '
                                 f'{stated[0][1]} and {given:g} was given; one of them is '
                                 'wrong, and nothing is resampled')
        if stated and stated[0][0] != given:
            notes.append(f'the file states {stated[0][0]:.10g} samples per second; '
                         f'{given:.10g}, as given, is written')
        return given
    if not stated:
        raise DriveFileError('the file states no rate; give the rate it was made at')
    return float(stated[0][0])


# ---- read ------------------------------------------------------------------------

def _sidecar(folder: str) -> dict[str, Any]:
    side = os.path.join(folder, SIDECAR_FILE)
    if not os.path.isfile(side):
        raise DriveFileError(f'{folder}: no {SIDECAR_FILE}; it is not a drive folder, or its '
                             'writing did not finish')
    with open(side, encoding='utf-8') as f:
        sidecar = json.load(f)
    if sidecar.get('format') != FORMAT or sidecar.get('version') != FORMAT_VERSION:
        raise DriveFileError(f'{folder}: not a version {FORMAT_VERSION} {FORMAT} folder')
    return sidecar


def read(folder: str | os.PathLike) -> DriveFile:
    """Read a drive folder, checked as the control service checks it: the
    magic, the size, the SHA-256 its sidecar records, and every value
    finite.

    Parameters
    ----------
    folder : str or os.PathLike
        The drive folder.

    Returns
    -------
    DriveFile
        Its values, `(samples, channels)` float64 volts, and its sidecar.

    Raises
    ------
    ValueError
        When a check fails, saying which.
    """
    folder = os.path.abspath(os.fspath(folder))
    sidecar = _sidecar(folder)
    names = [str(c['name']) for c in sidecar['channels']]
    samples, width = int(sidecar['samples']), len(names)
    binary = os.path.join(folder, BINARY_FILE)
    size = os.path.getsize(binary)
    if size != len(MAGIC) + SAMPLE_BYTES * width * samples:
        raise DriveFileError(f'{binary} is {size} bytes; {samples} samples of {width} '
                             f'channels are {len(MAGIC) + SAMPLE_BYTES * width * samples}')
    with open(binary, 'rb') as f:
        if f.read(len(MAGIC)) != MAGIC:
            raise DriveFileError(f'{binary} does not start as a drive file')
    if _file_sha256(binary) != sidecar['sha256']:
        raise DriveFileError(f'{binary} does not match the SHA-256 its sidecar records; '
                             'it changed after it was written')
    values = np.fromfile(binary, dtype=SAMPLE_DTYPE, offset=len(MAGIC)).reshape(samples, width)
    if not np.all(np.isfinite(values)):
        sample, channel = (int(v[0]) for v in np.nonzero(~np.isfinite(values)))
        raise DriveFileError(f'{names[channel]}: sample {sample + 1} is not a finite number')
    return DriveFile(folder, values, float(sidecar['rate_hz']), names, sidecar)


# ---- fix ends --------------------------------------------------------------------

def _integrals(x: np.ndarray, dt: float) -> tuple[np.ndarray, np.ndarray]:
    """Velocity and displacement of `x` taken as an acceleration: a plain
    trapezoid from rest, no high-pass."""
    v = np.concatenate(([0.0], np.cumsum((x[1:] + x[:-1]) * (0.5 * dt))))
    d = np.concatenate(([0.0], np.cumsum((v[1:] + v[:-1]) * (0.5 * dt))))
    return v, d


def _ends(x: np.ndarray, dt: float) -> dict[str, float]:
    v, d = _integrals(x, dt)
    return {'start': float(x[0]), 'end': float(x[-1]), 'peak': float(np.max(np.abs(x))),
            'velocity_end': float(v[-1]), 'velocity_peak': float(np.max(np.abs(v))),
            'displacement_end': float(d[-1]),
            'displacement_peak': float(np.max(np.abs(d)))}


def _rms(x: np.ndarray) -> float:
    return float(np.sqrt(np.mean(np.square(x))))


def taper_samples(samples: int, rate_hz: float, taper_s: float | None = None) -> int:
    """The samples of `fix_ends`' taper at each end: `taper_s`, or by
    default `TAPER_FRACTION` of the record within `TAPER_MIN_S` and
    `TAPER_MAX_S`; at most half the record.

    Parameters
    ----------
    samples : int
        The record's length.
    rate_hz : float
        Its rate.
    taper_s : float, optional
        The taper in seconds.

    Returns
    -------
    int
        The taper's length in samples.
    """
    if taper_s is None:
        duration = (samples - 1) / rate_hz
        taper_s = min(TAPER_MAX_S, max(TAPER_MIN_S, TAPER_FRACTION * duration))
    if not np.isfinite(taper_s) or taper_s < 0:
        raise DriveFileError(f'the taper {taper_s!r} is not a length of time')
    return int(min(round(taper_s * rate_hz), (samples - 1) // 2))


def _fix(x: np.ndarray, dt: float, taper: int, shapes: np.ndarray,
         matrix: np.ndarray) -> tuple[np.ndarray, float, float]:
    """One channel tapered and corrected, as stored (float64), with `a`
    and `b`."""
    y = x.copy()
    if taper > 0:
        w = 0.5 * (1.0 - np.cos(np.pi * np.arange(taper + 1) / taper))
        y[:taper + 1] *= w
        y[-taper - 1:] *= w[::-1]
    a = b = 0.0
    out = y
    # solved on the tapered record, then once more on the corrected record
    # exactly as it is stored, so what is written ends at rest
    for _pass in range(2):
        v, d = _integrals(out, dt)
        da, db = np.linalg.solve(matrix, [v[-1], d[-1]])
        a, b = a + float(da), b + float(db)
        out = y - a * shapes[0] - b * shapes[1]
    return out, a, b


def fix_ends(folder: str | os.PathLike, *, taper_s: float | None = None,
             out: str | os.PathLike | None = None,
             created_utc: str | None = None) -> dict[str, Any]:
    """Write a copy of a drive folder whose ends are at rest.

    Each channel is tapered at both ends with a half-cosine, then
    `c(t) = a sin(pi t / T) + b sin(2 pi t / T)` (T the record's length,
    so `c` is zero at both ends) is subtracted, `a` and `b` solved so the
    record, integrated as an acceleration with a plain trapezoid, ends
    with zero velocity and zero displacement. A channel that is zero
    throughout is left as it is. The original folder is not changed.

    Parameters
    ----------
    folder : str or os.PathLike
        The drive folder to correct.
    taper_s : float, optional
        The taper at each end, seconds; `taper_samples` by default.
    out : str or os.PathLike, optional
        The folder to make; refused when it exists. Beside `folder`,
        named `<name>-fixed`, when omitted.
    created_utc : str, optional
        The new sidecar's time, now when omitted.

    Returns
    -------
    dict
        `folder`, `drive` (the new sidecar), `from` (the original folder
        and its SHA-256), `taper_s`, `taper_samples`, and per channel its
        `name`, `a`, `b`, `rms_before`, `rms_after`, and `before` and
        `after`, each `start`, `end`, `peak`, `velocity_end`,
        `velocity_peak`, `displacement_end` and `displacement_peak` (volts,
        volt seconds and volt seconds squared).
    """
    original = read(folder)
    samples, rate = original.values.shape[0], original.rate_hz
    if samples < 4:
        raise DriveFileError(f'{original.folder}: {samples} samples are too few to fix')
    dt = 1.0 / rate
    taper = taper_samples(samples, rate, taper_s)
    t = np.arange(samples) / (samples - 1)
    shapes = np.vstack((np.sin(np.pi * t), np.sin(2 * np.pi * t)))
    shapes[:, 0] = shapes[:, -1] = 0.0
    m1 = _integrals(shapes[0], dt)
    m2 = _integrals(shapes[1], dt)
    matrix = np.array([[m1[0][-1], m2[0][-1]], [m1[1][-1], m2[1][-1]]])
    fixed = np.empty_like(original.values)
    channels = []
    for c, name in enumerate(original.names):
        x = original.values[:, c].astype(np.float64)
        if not np.any(x):
            fixed[:, c] = original.values[:, c]
            a = b = 0.0
        else:
            fixed[:, c], a, b = _fix(x, dt, taper, shapes, matrix)
        after = fixed[:, c].astype(np.float64)
        before_rms, after_rms = _rms(x), _rms(after)
        channels.append({'name': name, 'a': a, 'b': b, 'rms_before': before_rms,
                         'rms_after': after_rms, 'before': _ends(x, dt),
                         'after': _ends(after, dt)})
    side = original.sidecar
    record = {
        'kind': 'fix_ends',
        'from': {'folder': original.folder, 'sha256': side['sha256']},
        'taper_s': taper / rate,
        'taper_samples': taper,
        'correction': 'c(t) = a sin(pi t / T) + b sin(2 pi t / T), T = (samples - 1) / rate_hz',
        'channels': [{'name': ch['name'], 'a': ch['a'], 'b': ch['b'],
                      'rms_before': ch['rms_before'], 'rms_after': ch['rms_after']}
                     for ch in channels],
    }
    stem = os.path.basename(original.folder) + '-fixed'
    target = _claim(out, os.path.dirname(original.folder), stem)
    try:
        sidecar = _write_folder(target, iter([fixed]), samples, side['channels'], rate,
                                side['source'], [*side.get('fixes', []), record],
                                created_utc, None)
    except BaseException:
        shutil.rmtree(target, ignore_errors=True)
        raise
    return {'folder': target, 'drive': sidecar, 'from': record['from'],
            'taper_s': record['taper_s'], 'taper_samples': taper, 'channels': channels}


#: `fix_ends` by a name `convert`'s own `fix_ends` argument does not hide
_fix_ends = fix_ends
